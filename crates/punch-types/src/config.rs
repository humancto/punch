use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level Punch configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunchConfig {
    /// Socket address for the Arena API server (e.g. "127.0.0.1:6660").
    /// Use 127.0.0.1 for local-only access. Only bind to 0.0.0.0 if you
    /// need external access AND have authentication configured.
    pub api_listen: String,
    /// API key for authentication. If empty, auth is disabled (dev mode).
    #[serde(default)]
    pub api_key: String,
    /// Per-IP rate limit in requests per minute. Default: 60.
    #[serde(default = "default_rate_limit_rpm")]
    pub rate_limit_rpm: u32,
    /// Default model to use when none is specified.
    pub default_model: ModelConfig,
    /// Memory subsystem configuration.
    pub memory: MemoryConfig,
    /// Tunnel / public URL configuration shared by all channel webhooks.
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
    /// Channel configurations keyed by channel name (e.g. "slack", "discord").
    #[serde(default)]
    pub channels: HashMap<String, ChannelConfig>,
    /// MCP server definitions keyed by server name.
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// Smart model routing configuration. When enabled, messages are routed
    /// to cheap / mid / expensive models based on query complexity.
    #[serde(default)]
    pub model_routing: ModelRoutingConfig,
}

/// Configuration for a language model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The provider to use.
    pub provider: Provider,
    /// Model identifier (e.g. "claude-sonnet-4-20250514").
    pub model: String,
    /// Environment variable name that holds the API key.
    pub api_key_env: Option<String>,
    /// Optional base URL override for the provider API.
    pub base_url: Option<String>,
    /// Maximum tokens to generate per request.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
}

/// Supported model providers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    Google,
    #[serde(rename = "openai")]
    OpenAI,
    Groq,
    DeepSeek,
    Ollama,
    Mistral,
    Together,
    Fireworks,
    Cerebras,
    #[serde(rename = "xai")]
    XAI,
    Cohere,
    Bedrock,
    #[serde(rename = "azure_openai")]
    AzureOpenAi,
    Custom(String),
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::Google => write!(f, "google"),
            Self::OpenAI => write!(f, "openai"),
            Self::Groq => write!(f, "groq"),
            Self::DeepSeek => write!(f, "deepseek"),
            Self::Ollama => write!(f, "ollama"),
            Self::Mistral => write!(f, "mistral"),
            Self::Together => write!(f, "together"),
            Self::Fireworks => write!(f, "fireworks"),
            Self::Cerebras => write!(f, "cerebras"),
            Self::XAI => write!(f, "xai"),
            Self::Cohere => write!(f, "cohere"),
            Self::Bedrock => write!(f, "bedrock"),
            Self::AzureOpenAi => write!(f, "azure_openai"),
            Self::Custom(name) => write!(f, "custom({})", name),
        }
    }
}

/// Memory subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Path to the SQLite database file.
    pub db_path: String,
    /// Whether to enable the knowledge graph.
    #[serde(default = "default_true")]
    pub knowledge_graph_enabled: bool,
    /// Maximum number of memory entries to retain.
    pub max_entries: Option<u64>,
}

/// Configuration for the public tunnel / base URL used by all channel webhooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// The public base URL that all channel webhooks share
    /// (e.g. "https://abc.trycloudflare.com" or "https://channels.yourdomain.com").
    pub base_url: String,
    /// How this tunnel was set up: "quick", "named", or "manual".
    #[serde(default = "default_tunnel_mode")]
    pub mode: String,
}

fn default_tunnel_mode() -> String {
    "manual".to_string()
}

/// Configuration for a communication channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel type identifier (e.g. "slack", "discord", "webhook").
    pub channel_type: String,
    /// Environment variable holding the authentication token.
    pub token_env: Option<String>,
    /// Environment variable holding the webhook signing secret (for signature verification).
    /// Slack: signing secret. Telegram: secret_token header value. Discord: public key.
    pub webhook_secret_env: Option<String>,
    /// Allowlisted user/chat IDs. Only these users can interact with fighters.
    /// Empty list = open access (logs a security warning on startup).
    #[serde(default)]
    pub allowed_user_ids: Vec<String>,
    /// Per-user rate limit in messages per minute. Default: 20.
    #[serde(default = "default_channel_rate_limit")]
    pub rate_limit_per_user: u32,
    /// Additional channel-specific settings.
    #[serde(default)]
    pub settings: HashMap<String, serde_json::Value>,
}

fn default_channel_rate_limit() -> u32 {
    20
}

/// Configuration for an MCP (Model Context Protocol) server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to start the MCP server.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the MCP server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn default_rate_limit_rpm() -> u32 {
    60
}

/// Configuration for smart model routing based on query complexity.
///
/// When enabled, messages are classified into tiers (cheap / mid / expensive)
/// using keyword heuristics, and routed to the appropriate model. If a tier's
/// model is not configured, the default model is used as fallback.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelRoutingConfig {
    /// Whether model routing is enabled. When `false`, the default model is
    /// used for all messages (backward-compatible default).
    #[serde(default)]
    pub enabled: bool,
    /// Model for simple messages (greetings, yes/no answers). Cheap nano-tier.
    pub cheap: Option<ModelConfig>,
    /// Model for tool-calling messages (search, email, calendar, etc.).
    pub mid: Option<ModelConfig>,
    /// Model for complex reasoning (analysis, comparison, code review, etc.).
    pub expensive: Option<ModelConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_display_all_variants() {
        assert_eq!(Provider::Anthropic.to_string(), "anthropic");
        assert_eq!(Provider::Google.to_string(), "google");
        assert_eq!(Provider::OpenAI.to_string(), "openai");
        assert_eq!(Provider::Groq.to_string(), "groq");
        assert_eq!(Provider::DeepSeek.to_string(), "deepseek");
        assert_eq!(Provider::Ollama.to_string(), "ollama");
        assert_eq!(Provider::Mistral.to_string(), "mistral");
        assert_eq!(Provider::Together.to_string(), "together");
        assert_eq!(Provider::Fireworks.to_string(), "fireworks");
        assert_eq!(Provider::Cerebras.to_string(), "cerebras");
        assert_eq!(Provider::XAI.to_string(), "xai");
        assert_eq!(Provider::Cohere.to_string(), "cohere");
        assert_eq!(Provider::Bedrock.to_string(), "bedrock");
        assert_eq!(Provider::AzureOpenAi.to_string(), "azure_openai");
        assert_eq!(
            Provider::Custom("my_provider".to_string()).to_string(),
            "custom(my_provider)"
        );
    }

    #[test]
    fn test_provider_serde_roundtrip() {
        let providers = vec![
            Provider::Anthropic,
            Provider::Google,
            Provider::OpenAI,
            Provider::Groq,
            Provider::DeepSeek,
            Provider::Ollama,
            Provider::Mistral,
            Provider::Together,
            Provider::Fireworks,
            Provider::Cerebras,
            Provider::XAI,
            Provider::Cohere,
            Provider::Bedrock,
            Provider::AzureOpenAi,
            Provider::Custom("test".to_string()),
        ];
        for provider in &providers {
            let json = serde_json::to_string(provider).expect("serialize provider");
            let deser: Provider = serde_json::from_str(&json).expect("deserialize provider");
            assert_eq!(&deser, provider);
        }
    }

    #[test]
    fn test_provider_serde_rename() {
        assert_eq!(
            serde_json::to_string(&Provider::OpenAI).unwrap(),
            "\"openai\""
        );
        assert_eq!(serde_json::to_string(&Provider::XAI).unwrap(), "\"xai\"");
        assert_eq!(
            serde_json::to_string(&Provider::AzureOpenAi).unwrap(),
            "\"azure_openai\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Anthropic).unwrap(),
            "\"anthropic\""
        );
    }

    #[test]
    fn test_provider_equality() {
        assert_eq!(Provider::Anthropic, Provider::Anthropic);
        assert_ne!(Provider::Anthropic, Provider::Google);
        assert_eq!(
            Provider::Custom("x".to_string()),
            Provider::Custom("x".to_string())
        );
        assert_ne!(
            Provider::Custom("x".to_string()),
            Provider::Custom("y".to_string())
        );
    }

    #[test]
    fn test_model_config_serde_roundtrip() {
        let config = ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: ModelConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.model, "claude-sonnet-4-20250514");
        assert_eq!(deser.provider, Provider::Anthropic);
        assert_eq!(deser.max_tokens, Some(4096));
        assert_eq!(deser.temperature, Some(0.7));
    }

    #[test]
    fn test_model_config_optional_fields() {
        let config = ModelConfig {
            provider: Provider::Ollama,
            model: "llama3".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            max_tokens: None,
            temperature: None,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: ModelConfig = serde_json::from_str(&json).expect("deserialize");
        assert!(deser.api_key_env.is_none());
        assert_eq!(deser.base_url, Some("http://localhost:11434".to_string()));
        assert!(deser.max_tokens.is_none());
        assert!(deser.temperature.is_none());
    }

    #[test]
    fn test_memory_config_defaults() {
        let json = r#"{"db_path": "/tmp/punch.db"}"#;
        let config: MemoryConfig = serde_json::from_str(json).expect("deserialize");
        assert_eq!(config.db_path, "/tmp/punch.db");
        assert!(config.knowledge_graph_enabled); // default_true
        assert!(config.max_entries.is_none());
    }

    #[test]
    fn test_memory_config_roundtrip() {
        let config = MemoryConfig {
            db_path: "/data/punch.db".to_string(),
            knowledge_graph_enabled: false,
            max_entries: Some(10000),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: MemoryConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.db_path, "/data/punch.db");
        assert!(!deser.knowledge_graph_enabled);
        assert_eq!(deser.max_entries, Some(10000));
    }

    #[test]
    fn test_channel_config_serde() {
        let config = ChannelConfig {
            channel_type: "slack".to_string(),
            token_env: Some("SLACK_TOKEN".to_string()),
            webhook_secret_env: Some("SLACK_SIGNING_SECRET".to_string()),
            allowed_user_ids: vec!["U123".to_string()],
            rate_limit_per_user: 20,
            settings: HashMap::from([("channel_id".to_string(), serde_json::json!("C123456"))]),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: ChannelConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.channel_type, "slack");
        assert_eq!(deser.token_env, Some("SLACK_TOKEN".to_string()));
        assert_eq!(deser.settings["channel_id"], "C123456");
    }

    #[test]
    fn test_channel_config_defaults() {
        let json = r#"{"channel_type": "webhook"}"#;
        let config: ChannelConfig = serde_json::from_str(json).expect("deserialize");
        assert_eq!(config.channel_type, "webhook");
        assert!(config.token_env.is_none());
        assert!(config.settings.is_empty());
    }

    #[test]
    fn test_mcp_server_config_serde() {
        let config = McpServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server".to_string()],
            env: HashMap::from([("NODE_ENV".to_string(), "production".to_string())]),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: McpServerConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.command, "npx");
        assert_eq!(deser.args.len(), 2);
        assert_eq!(deser.env["NODE_ENV"], "production");
    }

    #[test]
    fn test_mcp_server_config_defaults() {
        let json = r#"{"command": "python"}"#;
        let config: McpServerConfig = serde_json::from_str(json).expect("deserialize");
        assert_eq!(config.command, "python");
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
    }

    #[test]
    fn test_tunnel_config_serde() {
        let config = TunnelConfig {
            base_url: "https://abc.trycloudflare.com".to_string(),
            mode: "quick".to_string(),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: TunnelConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.base_url, "https://abc.trycloudflare.com");
        assert_eq!(deser.mode, "quick");
    }

    #[test]
    fn test_tunnel_config_default_mode() {
        let json = r#"{"base_url": "https://example.com"}"#;
        let config: TunnelConfig = serde_json::from_str(json).expect("deserialize");
        assert_eq!(config.mode, "manual");
    }

    #[test]
    fn test_provider_hash() {
        let mut set = std::collections::HashSet::new();
        set.insert(Provider::Anthropic);
        set.insert(Provider::Google);
        set.insert(Provider::Anthropic); // duplicate
        assert_eq!(set.len(), 2);
    }
}
