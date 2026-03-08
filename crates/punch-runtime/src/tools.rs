//! Built-in tool definitions (JSON schemas for the LLM).
//!
//! This module defines the tool schemas that get sent to the LLM so it knows
//! what tools are available. The actual execution logic lives in `tool_executor`.

use punch_types::{Capability, ToolCategory, ToolDefinition};

/// Return all built-in tool definitions that match the given capabilities.
///
/// Only tools the fighter is allowed to use (based on granted capabilities) are
/// included. This prevents the LLM from seeing tools it can't invoke.
pub fn tools_for_capabilities(capabilities: &[Capability]) -> Vec<ToolDefinition> {
    let mut tools = Vec::new();

    for cap in capabilities {
        match cap {
            Capability::FileRead(_) => {
                push_unique(&mut tools, file_read());
                push_unique(&mut tools, file_list());
            }
            Capability::FileWrite(_) => {
                push_unique(&mut tools, file_write());
            }
            Capability::ShellExec(_) => {
                push_unique(&mut tools, shell_exec());
            }
            Capability::Network(_) => {
                push_unique(&mut tools, web_fetch());
            }
            Capability::Memory => {
                push_unique(&mut tools, memory_store());
                push_unique(&mut tools, memory_recall());
            }
            Capability::KnowledgeGraph => {
                push_unique(&mut tools, knowledge_add_entity());
                push_unique(&mut tools, knowledge_add_relation());
                push_unique(&mut tools, knowledge_query());
            }
            _ => {}
        }
    }

    tools
}

/// Return ALL built-in tool definitions (for unrestricted fighters).
pub fn all_tools() -> Vec<ToolDefinition> {
    vec![
        file_read(),
        file_write(),
        file_list(),
        shell_exec(),
        web_fetch(),
        memory_store(),
        memory_recall(),
        knowledge_add_entity(),
        knowledge_add_relation(),
        knowledge_query(),
    ]
}

fn push_unique(tools: &mut Vec<ToolDefinition>, tool: ToolDefinition) {
    if !tools.iter().any(|t| t.name == tool.name) {
        tools.push(tool);
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn file_read() -> ToolDefinition {
    ToolDefinition {
        name: "file_read".into(),
        description: "Read the contents of a file at the given path.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (relative to working directory or absolute)."
                }
            },
            "required": ["path"]
        }),
        category: ToolCategory::FileSystem,
    }
}

fn file_write() -> ToolDefinition {
    ToolDefinition {
        name: "file_write".into(),
        description:
            "Write content to a file at the given path. Creates parent directories if needed."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to."
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file."
                }
            },
            "required": ["path", "content"]
        }),
        category: ToolCategory::FileSystem,
    }
}

fn file_list() -> ToolDefinition {
    ToolDefinition {
        name: "file_list".into(),
        description: "List files and directories at the given path.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list (defaults to working directory)."
                }
            }
        }),
        category: ToolCategory::FileSystem,
    }
}

fn shell_exec() -> ToolDefinition {
    ToolDefinition {
        name: "shell_exec".into(),
        description: "Execute a shell command and return stdout, stderr, and exit code.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                }
            },
            "required": ["command"]
        }),
        category: ToolCategory::Shell,
    }
}

fn web_fetch() -> ToolDefinition {
    ToolDefinition {
        name: "web_fetch".into(),
        description: "Fetch the content of a URL via HTTP GET.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                }
            },
            "required": ["url"]
        }),
        category: ToolCategory::Web,
    }
}

fn memory_store() -> ToolDefinition {
    ToolDefinition {
        name: "memory_store".into(),
        description: "Store a key-value pair in your persistent memory. Use this to remember important facts, user preferences, or context across conversations.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "A short descriptive key for the memory."
                },
                "value": {
                    "type": "string",
                    "description": "The value to remember."
                },
                "confidence": {
                    "type": "number",
                    "description": "Confidence level from 0.0 to 1.0 (default: 0.9)."
                }
            },
            "required": ["key", "value"]
        }),
        category: ToolCategory::Memory,
    }
}

fn memory_recall() -> ToolDefinition {
    ToolDefinition {
        name: "memory_recall".into(),
        description: "Search your persistent memory for previously stored information.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find relevant memories."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 10)."
                }
            },
            "required": ["query"]
        }),
        category: ToolCategory::Memory,
    }
}

fn knowledge_add_entity() -> ToolDefinition {
    ToolDefinition {
        name: "knowledge_add_entity".into(),
        description: "Add an entity to your knowledge graph.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the entity."
                },
                "entity_type": {
                    "type": "string",
                    "description": "Type of entity (e.g. 'person', 'company', 'concept')."
                },
                "properties": {
                    "type": "object",
                    "description": "Additional properties as key-value pairs."
                }
            },
            "required": ["name", "entity_type"]
        }),
        category: ToolCategory::Knowledge,
    }
}

fn knowledge_add_relation() -> ToolDefinition {
    ToolDefinition {
        name: "knowledge_add_relation".into(),
        description: "Add a relation between two entities in your knowledge graph.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Source entity name."
                },
                "relation": {
                    "type": "string",
                    "description": "The relation type (e.g. 'works_at', 'depends_on')."
                },
                "to": {
                    "type": "string",
                    "description": "Target entity name."
                },
                "properties": {
                    "type": "object",
                    "description": "Additional properties."
                }
            },
            "required": ["from", "relation", "to"]
        }),
        category: ToolCategory::Knowledge,
    }
}

fn knowledge_query() -> ToolDefinition {
    ToolDefinition {
        name: "knowledge_query".into(),
        description: "Search your knowledge graph for entities and their relations.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find entities."
                }
            },
            "required": ["query"]
        }),
        category: ToolCategory::Knowledge,
    }
}
