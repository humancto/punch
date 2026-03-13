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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_display() {
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::Tool.to_string(), "tool");
    }

    #[test]
    fn test_role_serde_roundtrip() {
        let roles = vec![Role::User, Role::Assistant, Role::System, Role::Tool];
        for role in &roles {
            let json = serde_json::to_string(role).expect("serialize");
            let deser: Role = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, role);
        }
    }

    #[test]
    fn test_role_serde_values() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn test_message_new() {
        let msg = Message::new(Role::User, "Hello world");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "Hello world");
        assert!(msg.tool_calls.is_empty());
        assert!(msg.tool_results.is_empty());
    }

    #[test]
    fn test_message_new_empty_content() {
        let msg = Message::new(Role::Assistant, "");
        assert_eq!(msg.content, "");
    }

    #[test]
    fn test_message_serde_roundtrip() {
        let msg = Message::new(Role::User, "test message");
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::User);
        assert_eq!(deser.content, "test message");
    }

    #[test]
    fn test_message_serde_skips_empty_vecs() {
        let msg = Message::new(Role::User, "hi");
        let json = serde_json::to_string(&msg).expect("serialize");
        // skip_serializing_if = "Vec::is_empty" means these fields should be absent
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_results"));
    }

    #[test]
    fn test_tool_call_serde() {
        let call = ToolCall {
            id: "call_123".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let json = serde_json::to_string(&call).expect("serialize");
        let deser: ToolCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, "call_123");
        assert_eq!(deser.name, "read_file");
        assert_eq!(deser.input["path"], "/tmp/test.txt");
    }

    #[test]
    fn test_tool_call_result_serde() {
        let result = ToolCallResult {
            id: "call_123".to_string(),
            content: "file contents here".to_string(),
            is_error: false,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deser: ToolCallResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, "call_123");
        assert_eq!(deser.content, "file contents here");
        assert!(!deser.is_error);
    }

    #[test]
    fn test_tool_call_result_error() {
        let result = ToolCallResult {
            id: "call_456".to_string(),
            content: "Permission denied".to_string(),
            is_error: true,
        };
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_call_result_is_error_default() {
        // is_error has #[serde(default)], so missing field should be false
        let json = r#"{"id": "x", "content": "ok"}"#;
        let result: ToolCallResult = serde_json::from_str(json).expect("deserialize");
        assert!(!result.is_error);
    }

    #[test]
    fn test_message_with_tool_calls() {
        let mut msg = Message::new(Role::Assistant, "Let me check that file");
        msg.tool_calls.push(ToolCall {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "main.rs"}),
        });
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("tool_calls"));
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.tool_calls.len(), 1);
        assert_eq!(deser.tool_calls[0].name, "read_file");
    }

    #[test]
    fn test_role_equality() {
        assert_eq!(Role::User, Role::User);
        assert_ne!(Role::User, Role::Assistant);
    }

    #[test]
    fn test_role_hash() {
        let mut set = std::collections::HashSet::new();
        set.insert(Role::User);
        set.insert(Role::Assistant);
        set.insert(Role::User);
        assert_eq!(set.len(), 2);
    }
}
