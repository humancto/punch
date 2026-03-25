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
    Data,
    CodeAnalysis,
    Archive,
    Template,
    Crypto,
    Plugin,
    Channel,
    SystemAutomation,
    UiAutomation,
    AppIntegration,
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
            Self::Data => write!(f, "data"),
            Self::CodeAnalysis => write!(f, "code_analysis"),
            Self::Archive => write!(f, "archive"),
            Self::Template => write!(f, "template"),
            Self::Crypto => write!(f, "crypto"),
            Self::Plugin => write!(f, "plugin"),
            Self::Channel => write!(f, "channel"),
            Self::SystemAutomation => write!(f, "system_automation"),
            Self::UiAutomation => write!(f, "ui_automation"),
            Self::AppIntegration => write!(f, "app_integration"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_category_display_all() {
        assert_eq!(ToolCategory::FileSystem.to_string(), "filesystem");
        assert_eq!(ToolCategory::Web.to_string(), "web");
        assert_eq!(ToolCategory::Shell.to_string(), "shell");
        assert_eq!(ToolCategory::Memory.to_string(), "memory");
        assert_eq!(ToolCategory::Knowledge.to_string(), "knowledge");
        assert_eq!(ToolCategory::Browser.to_string(), "browser");
        assert_eq!(ToolCategory::Agent.to_string(), "agent");
        assert_eq!(ToolCategory::Schedule.to_string(), "schedule");
        assert_eq!(ToolCategory::Event.to_string(), "event");
        assert_eq!(ToolCategory::Media.to_string(), "media");
        assert_eq!(ToolCategory::SourceControl.to_string(), "source_control");
        assert_eq!(ToolCategory::Container.to_string(), "container");
        assert_eq!(ToolCategory::Data.to_string(), "data");
        assert_eq!(ToolCategory::CodeAnalysis.to_string(), "code_analysis");
        assert_eq!(ToolCategory::Archive.to_string(), "archive");
        assert_eq!(ToolCategory::Template.to_string(), "template");
        assert_eq!(ToolCategory::Crypto.to_string(), "crypto");
        assert_eq!(ToolCategory::Plugin.to_string(), "plugin");
        assert_eq!(ToolCategory::Channel.to_string(), "channel");
        assert_eq!(
            ToolCategory::SystemAutomation.to_string(),
            "system_automation"
        );
        assert_eq!(ToolCategory::UiAutomation.to_string(), "ui_automation");
        assert_eq!(ToolCategory::AppIntegration.to_string(), "app_integration");
    }

    #[test]
    fn test_tool_category_serde_roundtrip() {
        let categories = vec![
            ToolCategory::FileSystem,
            ToolCategory::Web,
            ToolCategory::Shell,
            ToolCategory::Memory,
            ToolCategory::Knowledge,
            ToolCategory::Browser,
            ToolCategory::Agent,
            ToolCategory::Schedule,
            ToolCategory::Event,
            ToolCategory::Media,
            ToolCategory::SourceControl,
            ToolCategory::Container,
            ToolCategory::Data,
            ToolCategory::CodeAnalysis,
            ToolCategory::Archive,
            ToolCategory::Template,
            ToolCategory::Crypto,
            ToolCategory::Plugin,
            ToolCategory::Channel,
            ToolCategory::SystemAutomation,
            ToolCategory::UiAutomation,
            ToolCategory::AppIntegration,
        ];
        for cat in &categories {
            let json = serde_json::to_string(cat).expect("serialize");
            let deser: ToolCategory = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, cat);
        }
    }

    #[test]
    fn test_tool_category_serde_values() {
        assert_eq!(
            serde_json::to_string(&ToolCategory::FileSystem).unwrap(),
            "\"file_system\""
        );
        assert_eq!(
            serde_json::to_string(&ToolCategory::SourceControl).unwrap(),
            "\"source_control\""
        );
        assert_eq!(
            serde_json::to_string(&ToolCategory::CodeAnalysis).unwrap(),
            "\"code_analysis\""
        );
    }

    #[test]
    fn test_tool_definition_serde() {
        let def = ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file from disk".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
            category: ToolCategory::FileSystem,
        };
        let json = serde_json::to_string(&def).expect("serialize");
        let deser: ToolDefinition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "read_file");
        assert_eq!(deser.category, ToolCategory::FileSystem);
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult {
            success: true,
            output: serde_json::json!({"data": "hello"}),
            error: None,
            duration_ms: 50,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deser: ToolResult = serde_json::from_str(&json).expect("deserialize");
        assert!(deser.success);
        assert!(deser.error.is_none());
        assert_eq!(deser.duration_ms, 50);
    }

    #[test]
    fn test_tool_result_failure() {
        let result = ToolResult {
            success: false,
            output: serde_json::Value::Null,
            error: Some("file not found".to_string()),
            duration_ms: 5,
        };
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("file not found"));
    }

    #[test]
    fn test_tool_category_equality() {
        assert_eq!(ToolCategory::Web, ToolCategory::Web);
        assert_ne!(ToolCategory::Web, ToolCategory::Shell);
    }

    #[test]
    fn test_tool_category_hash() {
        let mut set = std::collections::HashSet::new();
        set.insert(ToolCategory::Web);
        set.insert(ToolCategory::Shell);
        set.insert(ToolCategory::Web);
        assert_eq!(set.len(), 2);
    }
}
