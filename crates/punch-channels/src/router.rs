//! Channel router — maps channel+user combinations to fighter IDs.
//!
//! Maintains conversation continuity: the same user on the same platform
//! always routes to the same fighter and bout.

use std::sync::Mutex;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::debug;

use punch_types::FighterId;

use crate::ChannelPlatform;

/// A routing key combining platform and user identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    /// The platform (e.g. "telegram", "discord").
    pub platform: String,
    /// The user ID on that platform.
    pub user_id: String,
}

impl RouteKey {
    pub fn new(platform: &ChannelPlatform, user_id: &str) -> Self {
        Self {
            platform: platform.to_string(),
            user_id: user_id.to_string(),
        }
    }
}

/// Configuration for a channel's default fighter template.
#[derive(Debug, Clone)]
pub struct ChannelRouteConfig {
    /// The default fighter name/template to use for new users on this channel.
    pub default_fighter: String,
    /// The platform this config applies to.
    pub platform: ChannelPlatform,
}

/// Routes incoming channel messages to the correct fighter.
///
/// Routing priority:
/// 1. Direct route (platform+user -> specific fighter)
/// 2. User default (user -> fighter, across platforms)
/// 3. Channel default (platform -> default fighter template)
/// 4. System default fighter
pub struct ChannelRouter {
    /// Direct routes: (platform, user_id) -> FighterId.
    direct_routes: DashMap<RouteKey, FighterId>,
    /// Per-user default fighter (keyed by user_id).
    user_defaults: DashMap<String, FighterId>,
    /// Per-channel default fighter template name.
    channel_defaults: DashMap<String, String>,
    /// System-wide default fighter ID.
    system_default: Mutex<Option<FighterId>>,
    /// Fighter name -> FighterId cache for template resolution.
    fighter_cache: DashMap<String, FighterId>,
}

impl ChannelRouter {
    /// Create a new router.
    pub fn new() -> Self {
        Self {
            direct_routes: DashMap::new(),
            user_defaults: DashMap::new(),
            channel_defaults: DashMap::new(),
            system_default: Mutex::new(None),
            fighter_cache: DashMap::new(),
        }
    }

    /// Set the system-wide default fighter.
    pub fn set_system_default(&self, fighter_id: FighterId) {
        *self.system_default.lock().unwrap() = Some(fighter_id);
    }

    /// Set a per-channel default fighter template name.
    pub fn set_channel_default(&self, platform: &ChannelPlatform, fighter_name: String) {
        self.channel_defaults
            .insert(platform.to_string(), fighter_name);
    }

    /// Set a direct route for a specific platform+user combination.
    pub fn set_direct_route(
        &self,
        platform: &ChannelPlatform,
        user_id: &str,
        fighter_id: FighterId,
    ) {
        let key = RouteKey::new(platform, user_id);
        self.direct_routes.insert(key, fighter_id);
        debug!(
            platform = %platform,
            user_id = %user_id,
            fighter_id = %fighter_id,
            "direct route set"
        );
    }

    /// Set a user's default fighter (across all platforms).
    pub fn set_user_default(&self, user_id: &str, fighter_id: FighterId) {
        self.user_defaults.insert(user_id.to_string(), fighter_id);
    }

    /// Register a fighter name -> ID mapping for resolution.
    pub fn register_fighter(&self, name: String, id: FighterId) {
        self.fighter_cache.insert(name, id);
    }

    /// Resolve which fighter should handle a message.
    ///
    /// Returns the fighter ID, or None if no route is configured.
    pub fn resolve(&self, platform: &ChannelPlatform, user_id: &str) -> Option<FighterId> {
        // 1. Check direct routes
        let key = RouteKey::new(platform, user_id);
        if let Some(fighter_id) = self.direct_routes.get(&key) {
            return Some(*fighter_id);
        }

        // 2. Check user defaults
        if let Some(fighter_id) = self.user_defaults.get(user_id) {
            return Some(*fighter_id);
        }

        // 3. Check channel defaults (resolve name to ID via cache)
        let platform_str = platform.to_string();
        if let Some(fighter_name) = self.channel_defaults.get(&platform_str)
            && let Some(fighter_id) = self.fighter_cache.get(fighter_name.value())
        {
            return Some(*fighter_id);
        }

        // 4. System default
        *self.system_default.lock().unwrap()
    }

    /// Get the default fighter template name for a channel.
    pub fn channel_default_name(&self, platform: &ChannelPlatform) -> Option<String> {
        self.channel_defaults
            .get(&platform.to_string())
            .map(|v| v.value().clone())
    }

    /// Check if a user already has a route.
    pub fn has_route(&self, platform: &ChannelPlatform, user_id: &str) -> bool {
        let key = RouteKey::new(platform, user_id);
        self.direct_routes.contains_key(&key) || self.user_defaults.contains_key(user_id)
    }

    /// Get all configured channel defaults.
    pub fn list_channel_defaults(&self) -> Vec<(String, String)> {
        self.channel_defaults
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Get statistics about the router.
    pub fn stats(&self) -> RouterStats {
        RouterStats {
            direct_routes: self.direct_routes.len(),
            user_defaults: self.user_defaults.len(),
            channel_defaults: self.channel_defaults.len(),
            registered_fighters: self.fighter_cache.len(),
        }
    }
}

impl Default for ChannelRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the router state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterStats {
    pub direct_routes: usize,
    pub user_defaults: usize,
    pub channel_defaults: usize,
    pub registered_fighters: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_priority_direct() {
        let router = ChannelRouter::new();
        let direct_id = FighterId::new();
        let default_id = FighterId::new();

        router.set_system_default(default_id);
        router.set_direct_route(&ChannelPlatform::Telegram, "user1", direct_id);

        // Direct route wins
        let resolved = router.resolve(&ChannelPlatform::Telegram, "user1");
        assert_eq!(resolved, Some(direct_id));

        // Unknown user gets system default
        let resolved = router.resolve(&ChannelPlatform::Telegram, "user2");
        assert_eq!(resolved, Some(default_id));
    }

    #[test]
    fn test_routing_priority_user_default() {
        let router = ChannelRouter::new();
        let user_fighter = FighterId::new();
        let system_fighter = FighterId::new();

        router.set_system_default(system_fighter);
        router.set_user_default("alice", user_fighter);

        // User default wins over system default
        let resolved = router.resolve(&ChannelPlatform::Discord, "alice");
        assert_eq!(resolved, Some(user_fighter));
    }

    #[test]
    fn test_routing_priority_channel_default() {
        let router = ChannelRouter::new();
        let channel_fighter = FighterId::new();

        router.register_fighter("oracle".to_string(), channel_fighter);
        router.set_channel_default(&ChannelPlatform::Telegram, "oracle".to_string());

        let resolved = router.resolve(&ChannelPlatform::Telegram, "any_user");
        assert_eq!(resolved, Some(channel_fighter));

        // Different platform has no default
        let resolved = router.resolve(&ChannelPlatform::Discord, "any_user");
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_no_route() {
        let router = ChannelRouter::new();
        let resolved = router.resolve(&ChannelPlatform::Telegram, "user1");
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_same_user_different_platforms() {
        let router = ChannelRouter::new();
        let tg_fighter = FighterId::new();
        let dc_fighter = FighterId::new();

        router.set_direct_route(&ChannelPlatform::Telegram, "user1", tg_fighter);
        router.set_direct_route(&ChannelPlatform::Discord, "user1", dc_fighter);

        let tg = router.resolve(&ChannelPlatform::Telegram, "user1");
        let dc = router.resolve(&ChannelPlatform::Discord, "user1");

        assert_eq!(tg, Some(tg_fighter));
        assert_eq!(dc, Some(dc_fighter));
        assert_ne!(tg_fighter, dc_fighter);
    }

    #[test]
    fn test_has_route() {
        let router = ChannelRouter::new();
        let fighter_id = FighterId::new();

        assert!(!router.has_route(&ChannelPlatform::Telegram, "user1"));

        router.set_direct_route(&ChannelPlatform::Telegram, "user1", fighter_id);
        assert!(router.has_route(&ChannelPlatform::Telegram, "user1"));
    }

    #[test]
    fn test_stats() {
        let router = ChannelRouter::new();
        let id = FighterId::new();

        router.set_direct_route(&ChannelPlatform::Telegram, "u1", id);
        router.set_user_default("u2", id);
        router.set_channel_default(&ChannelPlatform::Discord, "bot".to_string());
        router.register_fighter("bot".to_string(), id);

        let stats = router.stats();
        assert_eq!(stats.direct_routes, 1);
        assert_eq!(stats.user_defaults, 1);
        assert_eq!(stats.channel_defaults, 1);
        assert_eq!(stats.registered_fighters, 1);
    }
}
