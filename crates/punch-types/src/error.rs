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

    #[error("troop error: {0}")]
    Troop(String),

    #[error("tenant error: {0}")]
    Tenant(String),

    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_display() {
        let err = PunchError::Config("missing api_key".to_string());
        assert_eq!(err.to_string(), "configuration error: missing api_key");
    }

    #[test]
    fn test_fighter_error_display() {
        let err = PunchError::Fighter("failed to spawn".to_string());
        assert_eq!(err.to_string(), "fighter error: failed to spawn");
    }

    #[test]
    fn test_gorilla_error_display() {
        let err = PunchError::Gorilla("schedule invalid".to_string());
        assert_eq!(err.to_string(), "gorilla error: schedule invalid");
    }

    #[test]
    fn test_bout_error_display() {
        let err = PunchError::Bout("session expired".to_string());
        assert_eq!(err.to_string(), "bout error: session expired");
    }

    #[test]
    fn test_capability_denied_display() {
        let err = PunchError::CapabilityDenied("file_write(/etc)".to_string());
        assert_eq!(err.to_string(), "capability denied: file_write(/etc)");
    }

    #[test]
    fn test_auth_error_display() {
        let err = PunchError::Auth("invalid token".to_string());
        assert_eq!(err.to_string(), "authentication error: invalid token");
    }

    #[test]
    fn test_tool_error_display() {
        let err = PunchError::Tool {
            tool: "web_fetch".to_string(),
            message: "timeout".to_string(),
        };
        assert_eq!(err.to_string(), "tool error [web_fetch]: timeout");
    }

    #[test]
    fn test_tool_not_found_display() {
        let err = PunchError::ToolNotFound("nonexistent_tool".to_string());
        assert_eq!(err.to_string(), "tool not found: nonexistent_tool");
    }

    #[test]
    fn test_tool_timeout_display() {
        let err = PunchError::ToolTimeout {
            tool: "shell_exec".to_string(),
            timeout_ms: 30000,
        };
        assert_eq!(err.to_string(), "tool timeout: shell_exec after 30000ms");
    }

    #[test]
    fn test_provider_error_display() {
        let err = PunchError::Provider {
            provider: "anthropic".to_string(),
            message: "server error".to_string(),
        };
        assert_eq!(err.to_string(), "provider error [anthropic]: server error");
    }

    #[test]
    fn test_rate_limited_display() {
        let err = PunchError::RateLimited {
            provider: "openai".to_string(),
            retry_after_ms: 5000,
        };
        assert_eq!(
            err.to_string(),
            "rate limited by openai, retry after 5000ms"
        );
    }

    #[test]
    fn test_context_overflow_display() {
        let err = PunchError::ContextOverflow {
            used: 150000,
            limit: 128000,
        };
        assert_eq!(
            err.to_string(),
            "model context length exceeded: 150000 / 128000 tokens"
        );
    }

    #[test]
    fn test_memory_error_display() {
        let err = PunchError::Memory("db locked".to_string());
        assert_eq!(err.to_string(), "memory error: db locked");
    }

    #[test]
    fn test_channel_error_display() {
        let err = PunchError::Channel {
            channel: "slack".to_string(),
            message: "auth failed".to_string(),
        };
        assert_eq!(err.to_string(), "channel error [slack]: auth failed");
    }

    #[test]
    fn test_event_bus_error_display() {
        let err = PunchError::EventBus("queue full".to_string());
        assert_eq!(err.to_string(), "event bus error: queue full");
    }

    #[test]
    fn test_mcp_error_display() {
        let err = PunchError::Mcp {
            server: "filesystem".to_string(),
            message: "connection refused".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "mcp error [filesystem]: connection refused"
        );
    }

    #[test]
    fn test_internal_error_display() {
        let err = PunchError::Internal("unexpected state".to_string());
        assert_eq!(err.to_string(), "internal error: unexpected state");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let punch_err: PunchError = io_err.into();
        assert!(punch_err.to_string().contains("file missing"));
        assert!(matches!(punch_err, PunchError::Io(_)));
    }

    #[test]
    fn test_from_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let punch_err: PunchError = serde_err.into();
        assert!(matches!(punch_err, PunchError::Serialization(_)));
    }

    #[test]
    fn test_punch_result_ok() {
        let result: PunchResult<i32> = Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_punch_result_err() {
        let result: PunchResult<i32> = Err(PunchError::Internal("fail".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_error_debug_impl() {
        let err = PunchError::Config("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("Config"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_empty_string_errors() {
        let err = PunchError::Config(String::new());
        assert_eq!(err.to_string(), "configuration error: ");
    }

    #[test]
    fn test_knowledge_graph_error_display() {
        let err = PunchError::KnowledgeGraph("cycle detected".to_string());
        assert_eq!(err.to_string(), "knowledge graph error: cycle detected");
    }
}
