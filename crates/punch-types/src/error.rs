use thiserror::Error;

/// Unified error type for the Punch system.
#[derive(Debug, Error)]
pub enum PunchError {
    // --- Core subsystem errors ---
    #[error("configuration error: {0}")]
    Config(String),

    #[error("fighter error: {0}")]
    Fighter(String),

    #[error("gorilla error: {0}")]
    Gorilla(String),

    #[error("bout error: {0}")]
    Bout(String),

    // --- Capability / auth errors ---
    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    #[error("authentication error: {0}")]
    Auth(String),

    // --- Tool / move errors ---
    #[error("tool error [{tool}]: {message}")]
    Tool { tool: String, message: String },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool timeout: {tool} after {timeout_ms}ms")]
    ToolTimeout { tool: String, timeout_ms: u64 },

    // --- Model / provider errors ---
    #[error("provider error [{provider}]: {message}")]
    Provider { provider: String, message: String },

    #[error("rate limited by {provider}, retry after {retry_after_ms}ms")]
    RateLimited {
        provider: String,
        retry_after_ms: u64,
    },

    #[error("model context length exceeded: {used} / {limit} tokens")]
    ContextOverflow { used: u64, limit: u64 },

    // --- Memory errors ---
    #[error("memory error: {0}")]
    Memory(String),

    #[error("knowledge graph error: {0}")]
    KnowledgeGraph(String),

    // --- Channel errors ---
    #[error("channel error [{channel}]: {message}")]
    Channel { channel: String, message: String },

    // --- Event errors ---
    #[error("event bus error: {0}")]
    EventBus(String),

    // --- MCP errors ---
    #[error("mcp error [{server}]: {message}")]
    Mcp { server: String, message: String },

    // --- I/O and infrastructure ---
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    // --- Catch-all ---
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias for `Result<T, PunchError>`.
pub type PunchResult<T> = Result<T, PunchError>;
