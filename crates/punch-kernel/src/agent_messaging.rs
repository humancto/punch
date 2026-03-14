//! # Inter-Agent Messaging
//!
//! Rich messaging between fighters using tokio channels.
//! Supports direct, broadcast, multicast, request-response, and streaming patterns.

use chrono::Utc;
use dashmap::DashMap;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;
use uuid::Uuid;

use punch_types::{
    AgentMessage, AgentMessageType, FighterId, MessageChannel, MessagePriority, PunchError,
    PunchResult,
};

/// Default mailbox capacity per fighter.
const DEFAULT_MAILBOX_CAPACITY: usize = 256;

/// Maximum dead letters to retain before oldest are dropped.
const MAX_DEAD_LETTERS: usize = 1000;

/// The messaging router handles delivery of inter-agent messages.
pub struct MessageRouter {
    /// Active mailboxes keyed by fighter ID.
    mailboxes: DashMap<FighterId, mpsc::Sender<AgentMessage>>,
    /// Receivers waiting to be claimed (fighter_id -> receiver).
    /// Using a DashMap with Option to allow one-time take.
    pending_receivers: DashMap<FighterId, mpsc::Receiver<AgentMessage>>,
    /// Dead letter queue for undeliverable messages.
    dead_letters: DashMap<u64, AgentMessage>,
    /// Counter for dead letter keys.
    dead_letter_counter: std::sync::atomic::AtomicU64,
    /// Pending request-response callbacks.
    pending_requests: DashMap<Uuid, oneshot::Sender<AgentMessage>>,
}

impl MessageRouter {
    /// Create a new message router.
    pub fn new() -> Self {
        Self {
            mailboxes: DashMap::new(),
            pending_receivers: DashMap::new(),
            dead_letters: DashMap::new(),
            dead_letter_counter: std::sync::atomic::AtomicU64::new(0),
            pending_requests: DashMap::new(),
        }
    }

    /// Register a fighter's mailbox. Returns a receiver for the fighter to
    /// consume messages from.
    pub fn register(&self, fighter_id: FighterId) -> mpsc::Receiver<AgentMessage> {
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        self.mailboxes.insert(fighter_id, tx);
        rx
    }

    /// Unregister a fighter's mailbox.
    pub fn unregister(&self, fighter_id: &FighterId) {
        self.mailboxes.remove(fighter_id);
        self.pending_receivers.remove(fighter_id);
    }

    /// Check if a fighter has a registered mailbox.
    pub fn is_registered(&self, fighter_id: &FighterId) -> bool {
        self.mailboxes.contains_key(fighter_id)
    }

    /// Send a direct message from one fighter to another.
    pub async fn send_direct(
        &self,
        from: FighterId,
        to: FighterId,
        content: AgentMessageType,
        priority: MessagePriority,
    ) -> PunchResult<Uuid> {
        let msg = AgentMessage {
            id: Uuid::new_v4(),
            from,
            to,
            channel: MessageChannel::Direct,
            content,
            priority,
            timestamp: Utc::now(),
            delivered: false,
        };

        self.deliver(msg).await
    }

    /// Broadcast a message to all registered fighters (except the sender).
    pub async fn broadcast(
        &self,
        from: FighterId,
        content: AgentMessageType,
        priority: MessagePriority,
    ) -> PunchResult<Vec<Uuid>> {
        let targets: Vec<FighterId> = self
            .mailboxes
            .iter()
            .map(|entry| *entry.key())
            .filter(|id| *id != from)
            .collect();

        let mut ids = Vec::new();
        for target in targets {
            let msg = AgentMessage {
                id: Uuid::new_v4(),
                from,
                to: target,
                channel: MessageChannel::Broadcast,
                content: content.clone(),
                priority,
                timestamp: Utc::now(),
                delivered: false,
            };
            match self.deliver(msg).await {
                Ok(id) => ids.push(id),
                Err(e) => warn!(target = %target, error = %e, "broadcast delivery failed"),
            }
        }

        Ok(ids)
    }

    /// Multicast a message to a specific set of fighters.
    pub async fn multicast(
        &self,
        from: FighterId,
        targets: Vec<FighterId>,
        content: AgentMessageType,
        priority: MessagePriority,
    ) -> PunchResult<Vec<Uuid>> {
        let mut ids = Vec::new();
        for target in &targets {
            let msg = AgentMessage {
                id: Uuid::new_v4(),
                from,
                to: *target,
                channel: MessageChannel::Multicast(targets.clone()),
                content: content.clone(),
                priority,
                timestamp: Utc::now(),
                delivered: false,
            };
            match self.deliver(msg).await {
                Ok(id) => ids.push(id),
                Err(e) => warn!(target = %target, error = %e, "multicast delivery failed"),
            }
        }

        Ok(ids)
    }

    /// Send a request and wait for a response with timeout.
    ///
    /// Returns the response message on success, or a timeout error.
    pub async fn request(
        &self,
        from: FighterId,
        to: FighterId,
        content: AgentMessageType,
        timeout: Duration,
    ) -> PunchResult<AgentMessage> {
        let msg_id = Uuid::new_v4();
        let (resp_tx, resp_rx) = oneshot::channel();

        self.pending_requests.insert(msg_id, resp_tx);

        let msg = AgentMessage {
            id: msg_id,
            from,
            to,
            channel: MessageChannel::Request {
                timeout_ms: timeout.as_millis() as u64,
            },
            content,
            priority: MessagePriority::High,
            timestamp: Utc::now(),
            delivered: false,
        };

        self.deliver(msg).await?;

        match tokio::time::timeout(timeout, resp_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                self.pending_requests.remove(&msg_id);
                Err(PunchError::Internal(
                    "request channel closed before response".to_string(),
                ))
            }
            Err(_) => {
                self.pending_requests.remove(&msg_id);
                Err(PunchError::Internal(format!(
                    "request timed out after {}ms",
                    timeout.as_millis()
                )))
            }
        }
    }

    /// Respond to a request message.
    pub fn respond(&self, original_msg_id: &Uuid, response: AgentMessage) -> PunchResult<()> {
        let (_, tx) = self
            .pending_requests
            .remove(original_msg_id)
            .ok_or_else(|| {
                PunchError::Internal(format!(
                    "no pending request for message {}",
                    original_msg_id
                ))
            })?;

        tx.send(response).map_err(|_| {
            PunchError::Internal("failed to send response: requester dropped".to_string())
        })
    }

    /// Internal delivery to a fighter's mailbox.
    async fn deliver(&self, msg: AgentMessage) -> PunchResult<Uuid> {
        let msg_id = msg.id;
        let target = msg.to;

        if let Some(tx) = self.mailboxes.get(&target) {
            match tx.try_send(msg) {
                Ok(()) => Ok(msg_id),
                Err(mpsc::error::TrySendError::Full(returned_msg)) => {
                    warn!(to = %target, "mailbox full, message queued as dead letter");
                    self.add_dead_letter(returned_msg);
                    Err(PunchError::Internal(format!(
                        "mailbox full for fighter {}",
                        target
                    )))
                }
                Err(mpsc::error::TrySendError::Closed(returned_msg)) => {
                    warn!(to = %target, "mailbox closed, message queued as dead letter");
                    self.add_dead_letter(returned_msg);
                    Err(PunchError::Internal(format!(
                        "mailbox closed for fighter {}",
                        target
                    )))
                }
            }
        } else {
            self.add_dead_letter(msg);
            Err(PunchError::Internal(format!(
                "no mailbox registered for fighter {}",
                target
            )))
        }
    }

    /// Add a message to the dead letter queue.
    fn add_dead_letter(&self, msg: AgentMessage) {
        let key = self
            .dead_letter_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.dead_letters.insert(key, msg);

        // Prune oldest if over limit.
        while self.dead_letters.len() > MAX_DEAD_LETTERS {
            // Remove the smallest key (oldest).
            if let Some(oldest) = self.dead_letters.iter().map(|e| *e.key()).min() {
                self.dead_letters.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Get the count of dead letters.
    pub fn dead_letter_count(&self) -> usize {
        self.dead_letters.len()
    }

    /// Drain all dead letters.
    pub fn drain_dead_letters(&self) -> Vec<AgentMessage> {
        let keys: Vec<u64> = self.dead_letters.iter().map(|e| *e.key()).collect();
        let mut messages = Vec::new();
        for key in keys {
            if let Some((_, msg)) = self.dead_letters.remove(&key) {
                messages.push(msg);
            }
        }
        messages
    }

    /// Get the number of registered mailboxes.
    pub fn registered_count(&self) -> usize {
        self.mailboxes.len()
    }
}

impl Default for MessageRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_receive() {
        let router = MessageRouter::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let mut rx1 = router.register(f1);
        let _rx2 = router.register(f2);

        let msg_id = router
            .send_direct(
                f2,
                f1,
                AgentMessageType::StatusUpdate {
                    progress: 1.0,
                    detail: "done".to_string(),
                },
                MessagePriority::Normal,
            )
            .await
            .expect("should deliver");

        let received = rx1.recv().await.expect("should receive");
        assert_eq!(received.id, msg_id);
        assert_eq!(received.from, f2);
    }

    #[tokio::test]
    async fn test_broadcast() {
        let router = MessageRouter::new();
        let sender = FighterId::new();
        let r1 = FighterId::new();
        let r2 = FighterId::new();
        let _sender_rx = router.register(sender);
        let mut rx1 = router.register(r1);
        let mut rx2 = router.register(r2);

        let ids = router
            .broadcast(
                sender,
                AgentMessageType::StatusUpdate {
                    progress: 0.5,
                    detail: "update".to_string(),
                },
                MessagePriority::Normal,
            )
            .await
            .expect("should broadcast");

        assert_eq!(ids.len(), 2);

        let m1 = rx1.recv().await.expect("should receive");
        let m2 = rx2.recv().await.expect("should receive");
        assert_eq!(m1.from, sender);
        assert_eq!(m2.from, sender);
    }

    #[tokio::test]
    async fn test_multicast() {
        let router = MessageRouter::new();
        let sender = FighterId::new();
        let t1 = FighterId::new();
        let t2 = FighterId::new();
        let t3 = FighterId::new();
        let _sr = router.register(sender);
        let mut rx1 = router.register(t1);
        let mut rx2 = router.register(t2);
        let _rx3 = router.register(t3);

        let ids = router
            .multicast(
                sender,
                vec![t1, t2],
                AgentMessageType::TaskAssignment {
                    task: "work".to_string(),
                },
                MessagePriority::High,
            )
            .await
            .expect("should multicast");

        assert_eq!(ids.len(), 2);

        let m1 = rx1.recv().await.expect("r1 should receive");
        let m2 = rx2.recv().await.expect("r2 should receive");
        assert_eq!(m1.from, sender);
        assert_eq!(m2.from, sender);
    }

    #[tokio::test]
    async fn test_request_response() {
        let router = std::sync::Arc::new(MessageRouter::new());
        let requester = FighterId::new();
        let responder = FighterId::new();
        let _req_rx = router.register(requester);
        let mut resp_rx = router.register(responder);

        let router_clone = router.clone();
        let requester_clone = requester;
        let responder_clone = responder;

        // Spawn responder task.
        tokio::spawn(async move {
            if let Some(msg) = resp_rx.recv().await {
                let response = AgentMessage {
                    id: Uuid::new_v4(),
                    from: responder_clone,
                    to: requester_clone,
                    channel: MessageChannel::Direct,
                    content: AgentMessageType::TaskResult {
                        result: "42".to_string(),
                        success: true,
                    },
                    priority: MessagePriority::Normal,
                    timestamp: Utc::now(),
                    delivered: false,
                };
                let _ = router_clone.respond(&msg.id, response);
            }
        });

        let result = router
            .request(
                requester,
                responder,
                AgentMessageType::TaskAssignment {
                    task: "compute".to_string(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("should get response");

        match &result.content {
            AgentMessageType::TaskResult { result, success } => {
                assert_eq!(result, "42");
                assert!(success);
            }
            _ => panic!("wrong response type"),
        }
    }

    #[tokio::test]
    async fn test_request_timeout() {
        let router = MessageRouter::new();
        let requester = FighterId::new();
        let responder = FighterId::new();
        let _req_rx = router.register(requester);
        let _resp_rx = router.register(responder);

        // Don't spawn a responder, so this will timeout.
        let result = router
            .request(
                requester,
                responder,
                AgentMessageType::TaskAssignment {
                    task: "compute".to_string(),
                },
                Duration::from_millis(50),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"));
    }

    #[tokio::test]
    async fn test_dead_letter_on_unregistered() {
        let router = MessageRouter::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let _rx = router.register(f1);

        // f2 is not registered; message should become dead letter.
        let result = router
            .send_direct(
                f1,
                f2,
                AgentMessageType::StatusUpdate {
                    progress: 0.0,
                    detail: "test".to_string(),
                },
                MessagePriority::Low,
            )
            .await;

        assert!(result.is_err());
        assert_eq!(router.dead_letter_count(), 1);
    }

    #[tokio::test]
    async fn test_drain_dead_letters() {
        let router = MessageRouter::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let _rx = router.register(f1);

        let _ = router
            .send_direct(
                f1,
                f2,
                AgentMessageType::StatusUpdate {
                    progress: 0.0,
                    detail: "dead".to_string(),
                },
                MessagePriority::Low,
            )
            .await;

        let letters = router.drain_dead_letters();
        assert_eq!(letters.len(), 1);
        assert_eq!(router.dead_letter_count(), 0);
    }

    #[test]
    fn test_unregister() {
        let router = MessageRouter::new();
        let f = FighterId::new();
        let _rx = router.register(f);
        assert!(router.is_registered(&f));
        router.unregister(&f);
        assert!(!router.is_registered(&f));
    }

    #[test]
    fn test_registered_count() {
        let router = MessageRouter::new();
        assert_eq!(router.registered_count(), 0);
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let _rx1 = router.register(f1);
        let _rx2 = router.register(f2);
        assert_eq!(router.registered_count(), 2);
    }

    #[tokio::test]
    async fn test_broadcast_excludes_sender() {
        let router = MessageRouter::new();
        let sender = FighterId::new();
        let mut sender_rx = router.register(sender);

        let ids = router
            .broadcast(
                sender,
                AgentMessageType::StatusUpdate {
                    progress: 1.0,
                    detail: "done".to_string(),
                },
                MessagePriority::Normal,
            )
            .await
            .expect("should broadcast");

        // No recipients besides sender, who is excluded.
        assert!(ids.is_empty());

        // Sender should NOT receive their own broadcast.
        let result = tokio::time::timeout(Duration::from_millis(50), sender_rx.recv()).await;
        assert!(result.is_err()); // Timeout means nothing received.
    }

    #[test]
    fn test_default_impl() {
        let router = MessageRouter::default();
        assert_eq!(router.registered_count(), 0);
    }

    #[tokio::test]
    async fn test_message_priority_preserved() {
        let router = MessageRouter::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let mut rx = router.register(f1);
        let _rx2 = router.register(f2);

        router
            .send_direct(
                f2,
                f1,
                AgentMessageType::Escalation {
                    reason: "urgent".to_string(),
                    original_task: "task".to_string(),
                },
                MessagePriority::Critical,
            )
            .await
            .expect("should deliver");

        let msg = rx.recv().await.expect("should receive");
        assert_eq!(msg.priority, MessagePriority::Critical);
    }

    #[tokio::test]
    async fn test_respond_to_nonexistent_request() {
        let router = MessageRouter::new();
        let response = AgentMessage {
            id: Uuid::new_v4(),
            from: FighterId::new(),
            to: FighterId::new(),
            channel: MessageChannel::Direct,
            content: AgentMessageType::TaskResult {
                result: "nope".to_string(),
                success: false,
            },
            priority: MessagePriority::Normal,
            timestamp: Utc::now(),
            delivered: false,
        };

        let result = router.respond(&Uuid::new_v4(), response);
        assert!(result.is_err());
    }
}
