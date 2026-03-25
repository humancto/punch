//! Smart model routing based on query complexity.
//!
//! Classifies user messages into tiers using keyword heuristics (no LLM call
//! required) and selects the appropriate model configuration for each tier.
//!
//! - **Cheap**: Simple greetings, yes/no, short acknowledgements. Nano models.
//! - **Mid**: Tool-calling messages (search, email, calendar, etc.).
//! - **Expensive**: Complex reasoning (analysis, comparison, code review, etc.).

use std::fmt;
use std::sync::Arc;

use tracing::debug;

use punch_types::config::{ModelConfig, ModelRoutingConfig};
use punch_types::{ContentPart, Message, PunchResult};

use crate::driver::{LlmDriver, create_driver};

/// The complexity tier for a user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    /// Simple responses, no tools needed. Suitable for nano models.
    Cheap,
    /// Tool calling required. Needs a model that reliably generates tool calls.
    Mid,
    /// Complex multi-step reasoning. Benefits from a frontier model.
    Expensive,
}

impl fmt::Display for ModelTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cheap => write!(f, "cheap"),
            Self::Mid => write!(f, "mid"),
            Self::Expensive => write!(f, "expensive"),
        }
    }
}

/// Keyword patterns that indicate expensive (complex reasoning) queries.
const EXPENSIVE_PATTERNS: &[&str] = &[
    "analyze",
    "compare",
    "summarize",
    "explain why",
    "write a",
    "create a plan",
    "review",
    "debug",
    "what are the pros and cons",
    "design",
    "refactor",
    "architect",
    "evaluate",
    "assess",
    "critique",
    "optimize",
    "trade-off",
    "tradeoff",
    "strategy",
    "deep dive",
];

/// Keyword patterns that indicate mid-tier (tool-calling) queries.
const TOOL_PATTERNS: &[&str] = &[
    "check", "calendar", "email", "send", "search", "find", "file", "download", "read", "schedule",
    "meeting", "remind", "weather", "stock", "price", "open", "run", "execute", "install", "list",
    "my ", "show me", "look up", "fetch", "get the", "delete", "update", "upload",
];

/// Smart model router that picks cheap / mid / expensive models based on
/// message complexity.
pub struct ModelRouter {
    config: ModelRoutingConfig,
}

impl ModelRouter {
    /// Create a new router from the routing configuration.
    pub fn new(config: ModelRoutingConfig) -> Self {
        Self { config }
    }

    /// Returns `true` if model routing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Classify a user message into a complexity tier using keyword heuristics.
    ///
    /// The classification is intentionally simple: pattern matching on the
    /// lowercased message. No LLM call is made.
    pub fn classify(message: &str) -> ModelTier {
        let lower = message.to_lowercase();

        // Check expensive patterns first (they take priority).
        if EXPENSIVE_PATTERNS.iter().any(|p| lower.contains(p)) {
            return ModelTier::Expensive;
        }

        // Check tool-calling patterns.
        if TOOL_PATTERNS.iter().any(|p| lower.contains(p)) {
            return ModelTier::Mid;
        }

        // Default: simple message, use cheap model.
        ModelTier::Cheap
    }

    /// Classify a user message with context awareness: if the conversation
    /// contains images (from screenshots, Telegram photos, etc.), force the
    /// expensive tier so a vision-capable model handles them.
    pub fn classify_with_context(message: &str, messages: &[Message]) -> ModelTier {
        // If any message in the conversation has an image, force expensive tier.
        let has_images = messages.iter().any(|m| {
            m.has_images()
                || m.content_parts
                    .iter()
                    .any(|p| matches!(p, ContentPart::Image { .. }))
                || m.tool_results.iter().any(|tr| tr.image.is_some())
        });
        if has_images {
            return ModelTier::Expensive;
        }

        // Also check tool results for png_base64 field (screenshot output).
        let has_screenshot_output = messages.iter().any(|m| {
            m.tool_results
                .iter()
                .any(|tr| tr.content.contains("png_base64"))
        });
        if has_screenshot_output {
            return ModelTier::Expensive;
        }

        Self::classify(message)
    }

    /// Select the model config for a given tier. Returns `None` if the tier
    /// has no model configured (caller should fall back to the default model).
    pub fn select_model(&self, tier: ModelTier) -> Option<&ModelConfig> {
        match tier {
            ModelTier::Cheap => self.config.cheap.as_ref(),
            ModelTier::Mid => self.config.mid.as_ref(),
            ModelTier::Expensive => self.config.expensive.as_ref(),
        }
    }

    /// Classify a message and create a driver for the selected tier.
    ///
    /// Returns `Some((tier, driver))` if routing is enabled and a tier-specific
    /// model is configured. Returns `None` if routing is disabled or the tier
    /// model is not configured (the caller should use the default driver).
    pub fn route_message(&self, message: &str) -> Option<(ModelTier, ModelConfig)> {
        if !self.config.enabled {
            return None;
        }

        let tier = Self::classify(message);
        let model_config = self.select_model(tier)?;

        debug!(
            tier = %tier,
            model = %model_config.model,
            provider = %model_config.provider,
            "model router selected"
        );

        Some((tier, model_config.clone()))
    }

    /// Create an LLM driver for a routed model config.
    pub fn create_tier_driver(config: &ModelConfig) -> PunchResult<Arc<dyn LlmDriver>> {
        create_driver(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::config::Provider;

    fn make_model_config(model: &str) -> ModelConfig {
        ModelConfig {
            provider: Provider::OpenAI,
            model: model.to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        }
    }

    fn make_routing_config(enabled: bool) -> ModelRoutingConfig {
        ModelRoutingConfig {
            enabled,
            cheap: Some(make_model_config("gpt-4.1-nano")),
            mid: Some(make_model_config("gpt-4.1-mini")),
            expensive: Some(make_model_config("gpt-4.1")),
        }
    }

    // -----------------------------------------------------------------------
    // Classification tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_classify_greeting_is_cheap() {
        assert_eq!(ModelRouter::classify("hello"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("hi there!"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("thanks"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("yes"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("no"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("ok"), ModelTier::Cheap);
        assert_eq!(ModelRouter::classify("good morning"), ModelTier::Cheap);
    }

    #[test]
    fn test_classify_tool_patterns_are_mid() {
        assert_eq!(ModelRouter::classify("check my email"), ModelTier::Mid);
        assert_eq!(
            ModelRouter::classify("search for rust tutorials"),
            ModelTier::Mid
        );
        assert_eq!(ModelRouter::classify("schedule a meeting"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("what's the weather"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("find the file"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("show me my calendar"), ModelTier::Mid);
        assert_eq!(
            ModelRouter::classify("send an email to Bob"),
            ModelTier::Mid
        );
        assert_eq!(ModelRouter::classify("download the report"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("list all files"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("run the tests"), ModelTier::Mid);
    }

    #[test]
    fn test_classify_complex_patterns_are_expensive() {
        assert_eq!(
            ModelRouter::classify("analyze this data"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("compare React vs Vue"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("summarize the article"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("explain why this fails"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("write a blog post"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("create a plan for migration"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("review this code"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("debug this issue"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("what are the pros and cons of microservices"),
            ModelTier::Expensive
        );
        assert_eq!(
            ModelRouter::classify("design a REST API"),
            ModelTier::Expensive
        );
    }

    #[test]
    fn test_classify_is_case_insensitive() {
        assert_eq!(ModelRouter::classify("ANALYZE this"), ModelTier::Expensive);
        assert_eq!(ModelRouter::classify("Check My Email"), ModelTier::Mid);
        assert_eq!(ModelRouter::classify("HELLO"), ModelTier::Cheap);
    }

    #[test]
    fn test_expensive_takes_priority_over_mid() {
        // "review" is expensive, "search" is mid — expensive should win.
        assert_eq!(
            ModelRouter::classify("review and search the codebase"),
            ModelTier::Expensive
        );
        // "analyze" is expensive, "find" is mid — expensive should win.
        assert_eq!(
            ModelRouter::classify("find and analyze the logs"),
            ModelTier::Expensive
        );
    }

    // -----------------------------------------------------------------------
    // Router selection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_select_model_returns_correct_tier() {
        let router = ModelRouter::new(make_routing_config(true));

        let cheap = router.select_model(ModelTier::Cheap).unwrap();
        assert_eq!(cheap.model, "gpt-4.1-nano");

        let mid = router.select_model(ModelTier::Mid).unwrap();
        assert_eq!(mid.model, "gpt-4.1-mini");

        let expensive = router.select_model(ModelTier::Expensive).unwrap();
        assert_eq!(expensive.model, "gpt-4.1");
    }

    #[test]
    fn test_select_model_returns_none_when_not_configured() {
        let config = ModelRoutingConfig {
            enabled: true,
            cheap: Some(make_model_config("gpt-4.1-nano")),
            mid: None,
            expensive: None,
        };
        let router = ModelRouter::new(config);

        assert!(router.select_model(ModelTier::Cheap).is_some());
        assert!(router.select_model(ModelTier::Mid).is_none());
        assert!(router.select_model(ModelTier::Expensive).is_none());
    }

    #[test]
    fn test_route_message_disabled() {
        let router = ModelRouter::new(make_routing_config(false));
        assert!(router.route_message("analyze this").is_none());
    }

    #[test]
    fn test_route_message_enabled() {
        let router = ModelRouter::new(make_routing_config(true));

        let (tier, config) = router.route_message("hello").unwrap();
        assert_eq!(tier, ModelTier::Cheap);
        assert_eq!(config.model, "gpt-4.1-nano");

        let (tier, config) = router.route_message("check my email").unwrap();
        assert_eq!(tier, ModelTier::Mid);
        assert_eq!(config.model, "gpt-4.1-mini");

        let (tier, config) = router.route_message("analyze the data").unwrap();
        assert_eq!(tier, ModelTier::Expensive);
        assert_eq!(config.model, "gpt-4.1");
    }

    #[test]
    fn test_route_message_falls_back_when_tier_missing() {
        let config = ModelRoutingConfig {
            enabled: true,
            cheap: None,
            mid: Some(make_model_config("gpt-4.1-mini")),
            expensive: None,
        };
        let router = ModelRouter::new(config);

        // Cheap tier not configured — returns None (caller uses default).
        assert!(router.route_message("hello").is_none());

        // Mid tier configured — returns Some.
        let result = router.route_message("search for files");
        assert!(result.is_some());

        // Expensive tier not configured — returns None.
        assert!(router.route_message("analyze this").is_none());
    }

    #[test]
    fn test_model_tier_display() {
        assert_eq!(ModelTier::Cheap.to_string(), "cheap");
        assert_eq!(ModelTier::Mid.to_string(), "mid");
        assert_eq!(ModelTier::Expensive.to_string(), "expensive");
    }

    #[test]
    fn test_default_routing_config_is_disabled() {
        let config = ModelRoutingConfig::default();
        assert!(!config.enabled);
        assert!(config.cheap.is_none());
        assert!(config.mid.is_none());
        assert!(config.expensive.is_none());
    }

    // -----------------------------------------------------------------------
    // Image detection tests (classify_with_context)
    // -----------------------------------------------------------------------

    #[test]
    fn test_classify_with_context_no_images_is_normal() {
        let messages = vec![Message::new(punch_types::Role::User, "hello")];
        assert_eq!(
            ModelRouter::classify_with_context("hello", &messages),
            ModelTier::Cheap
        );
    }

    #[test]
    fn test_classify_with_context_image_forces_expensive() {
        let msg = Message::with_parts(
            punch_types::Role::User,
            "What's in this image?",
            vec![ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "base64data".to_string(),
            }],
        );
        let messages = vec![msg];
        // Even though "hello" would be Cheap, image presence forces Expensive.
        assert_eq!(
            ModelRouter::classify_with_context("hello", &messages),
            ModelTier::Expensive
        );
    }

    #[test]
    fn test_classify_with_context_tool_result_image_forces_expensive() {
        let mut msg = Message::new(punch_types::Role::Tool, "");
        msg.tool_results = vec![punch_types::ToolCallResult {
            id: "tc1".to_string(),
            content: "screenshot taken".to_string(),
            is_error: false,
            image: Some(ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "base64data".to_string(),
            }),
        }];
        let messages = vec![msg];
        assert_eq!(
            ModelRouter::classify_with_context("ok", &messages),
            ModelTier::Expensive
        );
    }

    #[test]
    fn test_classify_with_context_png_base64_in_content() {
        let mut msg = Message::new(punch_types::Role::Tool, "");
        msg.tool_results = vec![punch_types::ToolCallResult {
            id: "tc1".to_string(),
            content: r#"{"png_base64": "iVBORw0KGgo=", "width": 1920}"#.to_string(),
            is_error: false,
            image: None,
        }];
        let messages = vec![msg];
        assert_eq!(
            ModelRouter::classify_with_context("ok", &messages),
            ModelTier::Expensive
        );
    }
}
