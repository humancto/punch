//! Channel bridge — connects channel adapters to the Ring via webhook handlers.
//!
//! Provides [`ChannelBridgeHandle`] (implemented by the Arena on the Ring) and
//! the [`process_incoming_message`] function that handles the full dispatch
//! pipeline: routing, fighter spawn, LLM call, response.

use async_trait::async_trait;

use punch_types::FighterId;

use crate::ChannelPlatform;
use crate::router::ChannelRouter;

/// Kernel operations needed by channel adapters.
///
/// Defined here to avoid circular deps (punch-channels cannot depend on
/// punch-kernel). Implemented in punch-api on the actual Ring.
#[async_trait]
pub trait ChannelBridgeHandle: Send + Sync {
    /// Send a message to a fighter and get the text response.
    async fn send_message(&self, fighter_id: FighterId, message: &str) -> Result<String, String>;

    /// Find a fighter by name, returning its ID.
    async fn find_fighter_by_name(&self, name: &str) -> Result<Option<FighterId>, String>;

    /// List running fighters as (id, name) pairs.
    async fn list_fighters(&self) -> Result<Vec<(FighterId, String)>, String>;

    /// Spawn a fighter by manifest name, returning its ID.
    async fn spawn_fighter_by_name(&self, manifest_name: &str) -> Result<FighterId, String>;
}

/// Process an incoming message from a channel webhook.
///
/// This is the main dispatch pipeline:
/// 1. Route the message to a fighter (via ChannelRouter)
/// 2. If no fighter exists for this user, spawn one using the channel's default
/// 3. Send the message to the fighter via the Ring
/// 4. Return the response text
///
/// Returns the response text to send back to the user.
pub async fn process_incoming_message(
    handle: &dyn ChannelBridgeHandle,
    router: &ChannelRouter,
    platform: &ChannelPlatform,
    user_id: &str,
    _display_name: &str,
    message_text: &str,
) -> Result<String, String> {
    // 1. Try to resolve an existing fighter for this user
    let fighter_id = match router.resolve(platform, user_id) {
        Some(id) => id,
        None => {
            // 2. No route exists — try to spawn a fighter from the channel default
            let default_name = router
                .channel_default_name(platform)
                .unwrap_or_else(|| "assistant".to_string());

            // Check if a fighter with that name already exists
            match handle.find_fighter_by_name(&default_name).await {
                Ok(Some(id)) => {
                    // Route this user to the existing fighter
                    router.set_direct_route(platform, user_id, id);
                    id
                }
                Ok(None) => {
                    // Spawn a new fighter
                    match handle.spawn_fighter_by_name(&default_name).await {
                        Ok(id) => {
                            router.set_direct_route(platform, user_id, id);
                            router.register_fighter(default_name, id);
                            id
                        }
                        Err(e) => {
                            // Try the first available fighter as fallback
                            match handle.list_fighters().await {
                                Ok(fighters) if !fighters.is_empty() => {
                                    let (id, _) = &fighters[0];
                                    router.set_direct_route(platform, user_id, *id);
                                    *id
                                }
                                _ => {
                                    return Err(format!(
                                        "No fighters available and could not spawn '{}': {}",
                                        default_name, e
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("Error finding fighter: {}", e));
                }
            }
        }
    };

    // 3. Send message to the fighter
    handle.send_message(fighter_id, message_text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockHandle {
        agents: Mutex<Vec<(FighterId, String)>>,
        responses: Mutex<Vec<(FighterId, String)>>,
    }

    #[async_trait]
    impl ChannelBridgeHandle for MockHandle {
        async fn send_message(
            &self,
            fighter_id: FighterId,
            message: &str,
        ) -> Result<String, String> {
            self.responses
                .lock()
                .unwrap()
                .push((fighter_id, message.to_string()));
            Ok(format!("Echo: {message}"))
        }

        async fn find_fighter_by_name(&self, name: &str) -> Result<Option<FighterId>, String> {
            let agents = self.agents.lock().unwrap();
            Ok(agents.iter().find(|(_, n)| n == name).map(|(id, _)| *id))
        }

        async fn list_fighters(&self) -> Result<Vec<(FighterId, String)>, String> {
            Ok(self.agents.lock().unwrap().clone())
        }

        async fn spawn_fighter_by_name(&self, _name: &str) -> Result<FighterId, String> {
            Err("spawn not implemented in mock".to_string())
        }
    }

    #[tokio::test]
    async fn test_process_message_existing_route() {
        let fighter_id = FighterId::new();
        let handle = MockHandle {
            agents: Mutex::new(vec![(fighter_id, "bot".to_string())]),
            responses: Mutex::new(Vec::new()),
        };

        let router = ChannelRouter::new();
        router.set_direct_route(&ChannelPlatform::Telegram, "user1", fighter_id);

        let result = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "Hello!",
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Echo: Hello!");
    }

    #[tokio::test]
    async fn test_process_message_auto_routes_to_existing_fighter() {
        let fighter_id = FighterId::new();
        let handle = MockHandle {
            agents: Mutex::new(vec![(fighter_id, "assistant".to_string())]),
            responses: Mutex::new(Vec::new()),
        };

        let router = ChannelRouter::new();

        // No route set — should find "assistant" and auto-route
        let result = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "Hello!",
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Echo: Hello!");

        // Verify the route was set for future messages
        assert!(router.has_route(&ChannelPlatform::Telegram, "user1"));
    }

    #[tokio::test]
    async fn test_process_message_no_fighters_available() {
        let handle = MockHandle {
            agents: Mutex::new(vec![]),
            responses: Mutex::new(Vec::new()),
        };

        let router = ChannelRouter::new();

        let result = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "Hello!",
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No fighters available"));
    }

    #[tokio::test]
    async fn test_conversation_continuity() {
        let fighter_id = FighterId::new();
        let handle = MockHandle {
            agents: Mutex::new(vec![(fighter_id, "assistant".to_string())]),
            responses: Mutex::new(Vec::new()),
        };

        let router = ChannelRouter::new();

        // First message creates the route
        let _ = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "First message",
        )
        .await;

        // Second message reuses the same route
        let _ = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "Second message",
        )
        .await;

        let responses = handle.responses.lock().unwrap();
        assert_eq!(responses.len(), 2);
        // Both messages went to the same fighter
        assert_eq!(responses[0].0, responses[1].0);
    }

    #[tokio::test]
    async fn test_different_users_different_routes() {
        let fighter_id = FighterId::new();
        let handle = MockHandle {
            agents: Mutex::new(vec![(fighter_id, "assistant".to_string())]),
            responses: Mutex::new(Vec::new()),
        };

        let router = ChannelRouter::new();

        // User1 sends a message
        let _ = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user1",
            "Alice",
            "Hello from user1",
        )
        .await;

        // User2 sends a message
        let _ = process_incoming_message(
            &handle,
            &router,
            &ChannelPlatform::Telegram,
            "user2",
            "Bob",
            "Hello from user2",
        )
        .await;

        // Both users should have routes set
        assert!(router.has_route(&ChannelPlatform::Telegram, "user1"));
        assert!(router.has_route(&ChannelPlatform::Telegram, "user2"));
    }
}
