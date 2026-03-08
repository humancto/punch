use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The role of a message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
            Self::System => write!(f, "system"),
            Self::Tool => write!(f, "tool"),
        }
    }
}

/// A message in a bout (conversation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message sender.
    pub role: Role,
    /// Text content of the message (may be empty for tool-only messages).
    pub content: String,
    /// Tool calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Results from tool executions (for role = Tool).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolCallResult>,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

impl Message {
    /// Create a simple text message with the current timestamp.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            timestamp: Utc::now(),
        }
    }
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// Input arguments as a JSON object.
    pub input: serde_json::Value,
}

/// The result of a tool call execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// The ID of the tool call this result corresponds to.
    pub id: String,
    /// Output content from the tool.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}
