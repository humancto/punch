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

    #[test]
    fn default_creates_event_bus() {
        let bus = EventBus::default();
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn clone_shares_same_channel() {
        let bus1 = EventBus::new();
        let bus2 = bus1.clone();
        let _rx = bus2.subscribe();
        // The subscriber from bus2 is visible on bus1 since they share the channel.
        assert_eq!(bus1.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn publish_payload_delivers_custom_payload() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let correlation = uuid::Uuid::new_v4();
        let payload = EventPayload::new(PunchEvent::Error {
            source: "custom".to_string(),
            message: "test payload".to_string(),
        })
        .with_correlation(correlation);

        let expected_id = payload.id;
        bus.publish_payload(payload);

        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_eq!(received.id, expected_id);
        assert_eq!(received.correlation_id, Some(correlation));
    }

    #[tokio::test]
    async fn with_capacity_creates_bus_with_custom_size() {
        let bus = EventBus::with_capacity(2);
        let mut rx = bus.subscribe();

        // Publish two events (within capacity).
        bus.publish(PunchEvent::Error {
            source: "a".to_string(),
            message: "1".to_string(),
        });
        bus.publish(PunchEvent::Error {
            source: "b".to_string(),
            message: "2".to_string(),
        });

        let p1 = rx.recv().await.unwrap();
        let p2 = rx.recv().await.unwrap();
        assert!(matches!(p1.event, PunchEvent::Error { .. }));
        assert!(matches!(p2.event, PunchEvent::Error { .. }));
    }

    #[tokio::test]
    async fn subscriber_receives_multiple_events_in_order() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        for i in 0..5 {
            bus.publish(PunchEvent::Error {
                source: "order-test".to_string(),
                message: format!("msg-{}", i),
            });
        }

        for i in 0..5 {
            let payload = rx.recv().await.unwrap();
            match &payload.event {
                PunchEvent::Error { message, .. } => {
                    assert_eq!(message, &format!("msg-{}", i));
                }
                _ => panic!("unexpected event"),
            }
        }
    }

    #[tokio::test]
    async fn filtered_receiver_skips_all_non_matching() {
        let bus = EventBus::new();
        let mut filtered =
            bus.subscribe_filtered(|event| matches!(event, PunchEvent::GorillaUnleashed { .. }));

        // Publish many non-matching events.
        for _ in 0..10 {
            bus.publish(PunchEvent::Error {
                source: "test".to_string(),
                message: "skip".to_string(),
            });
        }

        // Now the matching one.
        let gorilla_id = punch_types::GorillaId::new();
        bus.publish(PunchEvent::GorillaUnleashed {
            gorilla_id,
            name: "kong".to_string(),
        });

        let payload = timeout(Duration::from_secs(1), filtered.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        match &payload.event {
            PunchEvent::GorillaUnleashed { name, .. } => assert_eq!(name, "kong"),
            _ => panic!("wrong event"),
        }
    }

    #[tokio::test]
    async fn concurrent_publish_from_multiple_tasks() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let mut handles = Vec::new();
        for i in 0..10 {
            let bus_clone = bus.clone();
            handles.push(tokio::spawn(async move {
                bus_clone.publish(PunchEvent::Error {
                    source: format!("task-{}", i),
                    message: "concurrent".to_string(),
                });
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All 10 events should arrive.
        let mut count = 0;
        for _ in 0..10 {
            let _ = timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("timed out")
                .expect("channel closed");
            count += 1;
        }
        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn subscriber_dropped_does_not_affect_other_subscribers() {
        let bus = EventBus::new();
        let rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        drop(rx1);

        bus.publish(PunchEvent::Error {
            source: "test".to_string(),
            message: "after drop".to_string(),
        });

        let payload = rx2.recv().await.unwrap();
        match &payload.event {
            PunchEvent::Error { message, .. } => assert_eq!(message, "after drop"),
            _ => panic!("unexpected event"),
        }
    }

    #[tokio::test]
    async fn event_payload_has_timestamp() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let before = chrono::Utc::now();
        bus.publish(PunchEvent::Error {
            source: "ts".to_string(),
            message: "test".to_string(),
        });

        let payload = rx.recv().await.unwrap();
        assert!(payload.timestamp >= before);
    }
}
