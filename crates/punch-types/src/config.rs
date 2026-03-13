use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level Punch configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunchConfig {
    /// Socket address for the Arena API server (e.g. "0.0.0.0:6660").
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
    /// Channel configurations keyed by channel name (e.g. "slack", "discord").
    #[serde(default)]
    pub channels: HashMap<String, ChannelConfig>,
    /// MCP server definitions keyed by server name.
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
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

/// Configuration for a communication channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel type identifier (e.g. "slack", "discord", "webhook").
    pub channel_type: String,
    /// Environment variable holding the authentication token.
    pub token_env: Option<String>,
    /// Additional channel-specific settings.
    #[serde(default)]
    pub settings: HashMap<String, serde_json::Value>,
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
