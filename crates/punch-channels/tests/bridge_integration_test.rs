//! Integration tests for the channel bridge dispatch pipeline.
//!
//! These tests create a mock bridge handle, wire it through the router,
//! and verify the full dispatch pipeline works end-to-end.
//!
//! No external services are contacted — all communication is in-process.

use async_trait::async_trait;
use punch_channels::ChannelPlatform;
use punch_channels::bridge::{ChannelBridgeHandle, process_incoming_message};
use punch_channels::router::ChannelRouter;
use punch_types::FighterId;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock Bridge Handle — echoes messages, serves fighter lists
// ---------------------------------------------------------------------------

struct MockBridgeHandle {
    fighters: Mutex<Vec<(FighterId, String)>>,
    /// Records all messages sent to fighters: (fighter_id, message).
    received: Mutex<Vec<(FighterId, String)>>,
}

impl MockBridgeHandle {
    fn new(fighters: Vec<(FighterId, String)>) -> Self {
        Self {
            fighters: Mutex::new(fighters),
            received: Mutex::new(Vec::new()),
        }
    }

    fn get_received(&self) -> Vec<(FighterId, String)> {
        self.received.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChannelBridgeHandle for MockBridgeHandle {
    async fn send_message(
        &self,
        fighter_id: FighterId,
        message: &str,
        _image_parts: Vec<punch_types::ContentPart>,
    ) -> Result<String, String> {
        self.received
            .lock()
            .unwrap()
            .push((fighter_id, message.to_string()));
        Ok(format!("Echo: {message}"))
    }

    async fn find_fighter_by_name(&self, name: &str) -> Result<Option<FighterId>, String> {
        let fighters = self.fighters.lock().unwrap();
        Ok(fighters.iter().find(|(_, n)| n == name).map(|(id, _)| *id))
    }

    async fn list_fighters(&self) -> Result<Vec<(FighterId, String)>, String> {
        Ok(self.fighters.lock().unwrap().clone())
    }

    async fn spawn_fighter_by_name(&self, _name: &str) -> Result<FighterId, String> {
        Err("spawn not implemented in mock".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test: incoming message with pre-routed user gets echoed back.
#[tokio::test]
async fn test_bridge_dispatch_text_message() {
    let fighter_id = FighterId::new();
    let handle = MockBridgeHandle::new(vec![(fighter_id, "coder".to_string())]);
    let router = ChannelRouter::new();
    router.set_direct_route(&ChannelPlatform::Telegram, "user1", fighter_id);

    let response = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "user1",
        "Alice",
        "Hello agent!",
        vec![],
    )
    .await;

    assert!(response.is_ok());
    assert_eq!(response.unwrap(), "Echo: Hello agent!");

    // Verify the handle received the message for the correct fighter
    let received = handle.get_received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, fighter_id);
    assert_eq!(received[0].1, "Hello agent!");
}

/// Test: same user sends second message, reuses same fighter (conversation continuity).
#[tokio::test]
async fn test_conversation_continuity() {
    let fighter_id = FighterId::new();
    let handle = MockBridgeHandle::new(vec![(fighter_id, "assistant".to_string())]);
    let router = ChannelRouter::new();

    // First message — auto-routes to "assistant"
    let r1 = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "user1",
        "Alice",
        "First message",
        vec![],
    )
    .await;
    assert!(r1.is_ok());

    // Second message — should reuse the same fighter
    let r2 = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "user1",
        "Alice",
        "Second message",
        vec![],
    )
    .await;
    assert!(r2.is_ok());

    let received = handle.get_received();
    assert_eq!(received.len(), 2);
    // Both messages went to the same fighter
    assert_eq!(received[0].0, received[1].0);
    assert_eq!(received[0].0, fighter_id);
}

/// Test: different users get different routes (but may share the same fighter
/// if that's the only one available).
#[tokio::test]
async fn test_different_users_routed_independently() {
    let fighter_id = FighterId::new();
    let handle = MockBridgeHandle::new(vec![(fighter_id, "assistant".to_string())]);
    let router = ChannelRouter::new();

    // User1
    let _ = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "user1",
        "Alice",
        "Hello from user1",
        vec![],
    )
    .await;

    // User2
    let _ = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "user2",
        "Bob",
        "Hello from user2",
        vec![],
    )
    .await;

    // Both users should have routes
    assert!(router.has_route(&ChannelPlatform::Telegram, "user1"));
    assert!(router.has_route(&ChannelPlatform::Telegram, "user2"));

    // Both messages should have been processed
    let received = handle.get_received();
    assert_eq!(received.len(), 2);
}

/// Test: no fighters available returns an error.
#[tokio::test]
async fn test_no_fighters_available() {
    let handle = MockBridgeHandle::new(vec![]);
    let router = ChannelRouter::new();

    let result = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Discord,
        "user1",
        "Alice",
        "Hello!",
        vec![],
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No fighters available"));
}

/// Test: multiple platforms with different channel defaults.
#[tokio::test]
async fn test_multiple_platforms() {
    let tg_fighter = FighterId::new();
    let dc_fighter = FighterId::new();

    let handle = MockBridgeHandle::new(vec![
        (tg_fighter, "telegram-bot".to_string()),
        (dc_fighter, "discord-bot".to_string()),
    ]);

    let router = ChannelRouter::new();
    router.register_fighter("telegram-bot".to_string(), tg_fighter);
    router.register_fighter("discord-bot".to_string(), dc_fighter);
    router.set_channel_default(&ChannelPlatform::Telegram, "telegram-bot".to_string());
    router.set_channel_default(&ChannelPlatform::Discord, "discord-bot".to_string());

    // Telegram user
    let _ = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        "tg_user",
        "Alice",
        "from telegram",
        vec![],
    )
    .await;

    // Discord user
    let _ = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Discord,
        "dc_user",
        "Bob",
        "from discord",
        vec![],
    )
    .await;

    let received = handle.get_received();
    assert_eq!(received.len(), 2);
    // Telegram message went to tg_fighter
    assert_eq!(received[0].0, tg_fighter);
    assert_eq!(received[0].1, "from telegram");
    // Discord message went to dc_fighter
    assert_eq!(received[1].0, dc_fighter);
    assert_eq!(received[1].1, "from discord");
}

/// Test: fallback to first available fighter when no channel default matches.
#[tokio::test]
async fn test_fallback_to_first_fighter() {
    let fighter_id = FighterId::new();
    let handle = MockBridgeHandle::new(vec![(fighter_id, "general".to_string())]);
    let router = ChannelRouter::new();

    // No channel default set, no "assistant" fighter, but "general" exists as fallback
    let result = process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Slack,
        "user1",
        "Alice",
        "Hello!",
        vec![],
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Echo: Hello!");
}
