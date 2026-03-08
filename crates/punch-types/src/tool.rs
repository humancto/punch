use serde::{Deserialize, Serialize};

/// Category of a tool (move).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    FileSystem,
    Web,
    Shell,
    Memory,
    Knowledge,
    Browser,
    Agent,
    Schedule,
    Event,
    Media,
    SourceControl,
    Container,
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileSystem => write!(f, "filesystem"),
            Self::Web => write!(f, "web"),
            Self::Shell => write!(f, "shell"),
            Self::Memory => write!(f, "memory"),
            Self::Knowledge => write!(f, "knowledge"),
            Self::Browser => write!(f, "browser"),
            Self::Agent => write!(f, "agent"),
            Self::Schedule => write!(f, "schedule"),
            Self::Event => write!(f, "event"),
            Self::Media => write!(f, "media"),
            Self::SourceControl => write!(f, "source_control"),
            Self::Container => write!(f, "container"),
        }
    }
}

/// Definition of a tool (move) available to agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique name of the tool.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Category this tool belongs to.
    pub category: ToolCategory,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the tool execution succeeded.
    pub success: bool,
    /// The output content (may be structured JSON or plain text).
    pub output: serde_json::Value,
    /// Optional error message if the tool failed.
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}
