//! Event bus for publishing and subscribing to system-wide [`PunchEvent`]s.
//!
//! Built on top of [`tokio::sync::broadcast`] so that multiple subscribers can
//! independently receive every event without blocking the publisher.

use tokio::sync::broadcast;
use tracing::{debug, warn};

use punch_types::{EventPayload, PunchEvent};

/// Default channel capacity for the broadcast bus.
const DEFAULT_CAPACITY: usize = 1024;

/// A broadcast-based event bus for the Punch system.
///
/// Cloning an `EventBus` yields a handle to the **same** underlying channel.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventPayload>,
}

impl EventBus {
    /// Create a new event bus with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a new event bus with a specific channel capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publish an event to all active subscribers.
    ///
    /// If there are no subscribers the event is silently dropped.
    pub fn publish(&self, event: PunchEvent) {
        let payload = EventPayload::new(event);
        match self.sender.send(payload) {
            Ok(receivers) => {
                debug!(receivers, "event published");
            }
            Err(_) => {
                // No active receivers — this is not an error.
                debug!("event published with no active subscribers");
            }
        }
    }

    /// Publish a pre-built [`EventPayload`] (useful when you need a custom
    /// correlation ID or timestamp).
    pub fn publish_payload(&self, payload: EventPayload) {
        match self.sender.send(payload) {
            Ok(receivers) => {
                debug!(receivers, "event payload published");
            }
            Err(_) => {
                debug!("event payload published with no active subscribers");
            }
        }
    }

    /// Subscribe to all future events on this bus.
    ///
    /// Returns a [`broadcast::Receiver`] that will yield every event published
    /// after the subscription is created. If the receiver falls behind by more
    /// than the channel capacity, older events will be dropped and the receiver
    /// will see a [`broadcast::error::RecvError::Lagged`] error.
    pub fn subscribe(&self) -> broadcast::Receiver<EventPayload> {
        self.sender.subscribe()
    }

    /// Subscribe and return only events matching a predicate.
    ///
    /// This is a convenience wrapper — filtering happens on the receiver side.
    /// For high-throughput scenarios consider filtering inside the subscriber's
    /// own task loop instead.
    pub fn subscribe_filtered<F>(&self, predicate: F) -> FilteredReceiver<F>
    where
        F: Fn(&PunchEvent) -> bool + Send + 'static,
    {
        FilteredReceiver {
            inner: self.sender.subscribe(),
            predicate,
        }
    }

    /// Return the current number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// A receiver that applies a user-supplied predicate to incoming events.
pub struct FilteredReceiver<F>
where
    F: Fn(&PunchEvent) -> bool,
{
    inner: broadcast::Receiver<EventPayload>,
    predicate: F,
}

impl<F> FilteredReceiver<F>
where
    F: Fn(&PunchEvent) -> bool,
{
    /// Receive the next event that passes the filter.
    ///
    /// Skips events that do not match the predicate. Returns `None` when the
    /// channel is closed.
    pub async fn recv(&mut self) -> Option<EventPayload> {
        loop {
            match self.inner.recv().await {
                Ok(payload) => {
                    if (self.predicate)(&payload.event) {
                        return Some(payload);
                    }
                    // Skip non-matching events.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "filtered receiver lagged behind");
                    // Continue receiving after the gap.
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::FighterId;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn publish_and_receive_single_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let fighter_id = FighterId::new();
        bus.publish(PunchEvent::FighterSpawned {
            fighter_id,
            name: "test-fighter".to_string(),
        });

        let payload = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match &payload.event {
            PunchEvent::FighterSpawned { name, .. } => {
                assert_eq!(name, "test-fighter");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(PunchEvent::Error {
            source: "test".to_string(),
            message: "hello".to_string(),
        });

        let p1 = rx1.recv().await.unwrap();
        let p2 = rx2.recv().await.unwrap();

        assert_eq!(p1.id, p2.id);
    }

    #[tokio::test]
    async fn no_subscribers_does_not_panic() {
        let bus = EventBus::new();
        // Should not panic even with zero receivers.
        bus.publish(PunchEvent::Error {
            source: "test".to_string(),
            message: "nobody listening".to_string(),
        });
    }

    #[tokio::test]
    async fn filtered_receiver_only_gets_matching_events() {
        let bus = EventBus::new();
        let mut filtered =
            bus.subscribe_filtered(|event| matches!(event, PunchEvent::FighterSpawned { .. }));

        // Publish a non-matching event first.
        bus.publish(PunchEvent::Error {
            source: "test".to_string(),
            message: "should be skipped".to_string(),
        });

        // Then a matching event.
        let fighter_id = FighterId::new();
        bus.publish(PunchEvent::FighterSpawned {
            fighter_id,
            name: "filtered-fighter".to_string(),
        });

        let payload = timeout(Duration::from_secs(1), filtered.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match &payload.event {
            PunchEvent::FighterSpawned { name, .. } => {
                assert_eq!(name, "filtered-fighter");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[tokio::test]
    async fn subscriber_count_tracks_active_receivers() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(_rx1);
        assert_eq!(bus.subscriber_count(), 1);
    }
}
