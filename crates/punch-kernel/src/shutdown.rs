//! Graceful shutdown coordinator for the Punch kernel.
//!
//! Tracks in-flight requests, broadcasts a shutdown signal, waits for
//! requests to drain (with a configurable timeout), and fires registered
//! shutdown hooks in order.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tracing::{info, warn};

/// A callback that runs during shutdown. The boxed future must be `Send`.
pub type ShutdownHook = Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Coordinates graceful shutdown across the system.
///
/// # Shutdown phases
///
/// 1. **Stop accepting** — signal is broadcast, new requests get 503.
/// 2. **Drain in-flight** — wait for the in-flight counter to reach zero.
/// 3. **Cage all gorillas** — stop autonomous agents.
/// 4. **Flush logs** — give tracing subscribers time to flush.
/// 5. **Close DB** — release database handles.
///
/// Hooks registered via [`register_hook`](ShutdownCoordinator::register_hook)
/// fire in registration order after the drain phase.
pub struct ShutdownCoordinator {
    /// Sender half — set to `true` to broadcast shutdown.
    shutdown_signal: watch::Sender<bool>,
    /// Receiver half — subscribers clone this to watch for shutdown.
    shutdown_receiver: watch::Receiver<bool>,
    /// Number of requests currently in flight.
    in_flight: AtomicUsize,
    /// Maximum time to wait for in-flight requests to finish.
    drain_timeout: Duration,
    /// Ordered list of hooks to run during shutdown.
    hooks: Mutex<Vec<ShutdownHook>>,
    /// Whether shutdown has already been initiated (idempotency guard).
    initiated: AtomicBool,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator with the given drain timeout.
    pub fn new(drain_timeout: Duration) -> Arc<Self> {
        let (tx, rx) = watch::channel(false);
        Arc::new(Self {
            shutdown_signal: tx,
            shutdown_receiver: rx,
            in_flight: AtomicUsize::new(0),
            drain_timeout,
            hooks: Mutex::new(Vec::new()),
            initiated: AtomicBool::new(false),
        })
    }

    /// Create a coordinator with the default 30-second drain timeout.
    pub fn with_default_timeout() -> Arc<Self> {
        Self::new(Duration::from_secs(30))
    }

    /// Subscribe to the shutdown signal.
    ///
    /// The returned receiver yields `true` once shutdown is initiated.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_receiver.clone()
    }

    /// Returns `true` if shutdown has been initiated.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown_receiver.borrow()
    }

    /// Increment the in-flight request counter.
    ///
    /// Returns `false` if shutdown is in progress (caller should reject
    /// the request with 503).
    pub fn track_request(&self) -> bool {
        if self.is_shutting_down() {
            return false;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Decrement the in-flight request counter.
    pub fn finish_request(&self) {
        let prev = self.in_flight.fetch_sub(1, Ordering::SeqCst);
        // Guard against underflow (shouldn't happen, but be safe).
        if prev == 0 {
            self.in_flight.store(0, Ordering::SeqCst);
        }
    }

    /// Current number of in-flight requests.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Register a hook that will be called during shutdown.
    ///
    /// Hooks fire in registration order after the drain phase completes
    /// or times out.
    pub async fn register_hook<F, Fut>(&self, hook: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let boxed: ShutdownHook = Box::new(move || Box::pin(hook()));
        self.hooks.lock().await.push(boxed);
    }

    /// Initiate graceful shutdown.
    ///
    /// This method is idempotent — calling it multiple times has no
    /// additional effect.
    ///
    /// 1. Broadcasts the shutdown signal (new requests will be rejected).
    /// 2. Waits for in-flight requests to drain (up to `drain_timeout`).
    /// 3. Runs all registered hooks in order.
    pub async fn initiate_shutdown(&self) {
        // Idempotency: only run once.
        if self
            .initiated
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            info!("shutdown already in progress, ignoring duplicate signal");
            return;
        }

        info!("initiating graceful shutdown");

        // Phase 1: broadcast signal.
        let _ = self.shutdown_signal.send(true);

        // Phase 2: drain in-flight requests.
        let drain_start = tokio::time::Instant::now();
        let deadline = drain_start + self.drain_timeout;

        loop {
            let count = self.in_flight.load(Ordering::SeqCst);
            if count == 0 {
                info!("all in-flight requests drained");
                break;
            }

            if tokio::time::Instant::now() >= deadline {
                warn!(
                    remaining = count,
                    "drain timeout reached, force-terminating remaining requests"
                );
                break;
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 3-5: run hooks (cage gorillas, flush logs, close DB, etc.).
        let hooks = self.hooks.lock().await;
        for (i, hook) in hooks.iter().enumerate() {
            info!(hook_index = i, "running shutdown hook");
            hook().await;
        }

        info!("shutdown complete");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[tokio::test]
    async fn shutdown_signal_propagates() {
        let coord = ShutdownCoordinator::with_default_timeout();
        let mut rx = coord.subscribe();

        assert!(!coord.is_shutting_down());

        coord.initiate_shutdown().await;

        // Receiver should see the signal.
        rx.changed().await.ok();
        assert!(*rx.borrow());
        assert!(coord.is_shutting_down());
    }

    #[tokio::test]
    async fn in_flight_counter_tracks() {
        let coord = ShutdownCoordinator::with_default_timeout();

        assert_eq!(coord.in_flight_count(), 0);

        assert!(coord.track_request());
        assert_eq!(coord.in_flight_count(), 1);

        assert!(coord.track_request());
        assert_eq!(coord.in_flight_count(), 2);

        coord.finish_request();
        assert_eq!(coord.in_flight_count(), 1);

        coord.finish_request();
        assert_eq!(coord.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn drain_waits_for_in_flight() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(5));
        let coord_clone = Arc::clone(&coord);

        // Start a request.
        assert!(coord.track_request());

        // Start shutdown in the background.
        let handle = tokio::spawn(async move {
            coord_clone.initiate_shutdown().await;
        });

        // Give shutdown a moment to start draining.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Finish the request — shutdown should then complete.
        coord.finish_request();

        // Shutdown should complete within a reasonable time.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("shutdown should complete")
            .expect("shutdown task should not panic");
    }

    #[tokio::test]
    async fn drain_timeout_forces_shutdown() {
        let coord = ShutdownCoordinator::new(Duration::from_millis(100));

        // Start a request but never finish it.
        assert!(coord.track_request());

        let start = tokio::time::Instant::now();
        coord.initiate_shutdown().await;
        let elapsed = start.elapsed();

        // Should have timed out, not waited forever.
        assert!(elapsed < Duration::from_secs(2));
        // Request is still in flight (force-terminated).
        assert_eq!(coord.in_flight_count(), 1);
    }

    #[tokio::test]
    async fn hooks_fire_in_order() {
        let coord = ShutdownCoordinator::with_default_timeout();

        let order = Arc::new(Mutex::new(Vec::<u32>::new()));
        let o1 = Arc::clone(&order);
        let o2 = Arc::clone(&order);
        let o3 = Arc::clone(&order);

        coord
            .register_hook(move || {
                let o = Arc::clone(&o1);
                async move {
                    o.lock().await.push(1);
                }
            })
            .await;

        coord
            .register_hook(move || {
                let o = Arc::clone(&o2);
                async move {
                    o.lock().await.push(2);
                }
            })
            .await;

        coord
            .register_hook(move || {
                let o = Arc::clone(&o3);
                async move {
                    o.lock().await.push(3);
                }
            })
            .await;

        coord.initiate_shutdown().await;

        let fired = order.lock().await;
        assert_eq!(*fired, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn multiple_shutdown_signals_are_idempotent() {
        let coord = ShutdownCoordinator::with_default_timeout();
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        coord
            .register_hook(move || {
                let c = Arc::clone(&c);
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        coord.initiate_shutdown().await;
        coord.initiate_shutdown().await;
        coord.initiate_shutdown().await;

        // Hook should have fired exactly once.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn track_request_rejected_during_shutdown() {
        let coord = ShutdownCoordinator::with_default_timeout();

        coord.initiate_shutdown().await;

        // New requests should be rejected.
        assert!(!coord.track_request());
    }
}
