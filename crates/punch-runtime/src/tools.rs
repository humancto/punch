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
                push_unique(&mut tools, file_search());
                push_unique(&mut tools, file_info());
            }
            Capability::FileWrite(_) => {
                push_unique(&mut tools, file_write());
                push_unique(&mut tools, patch_apply());
            }
            Capability::ShellExec(_) => {
                push_unique(&mut tools, shell_exec());
                push_unique(&mut tools, process_list());
                push_unique(&mut tools, process_kill());
                push_unique(&mut tools, env_get());
                push_unique(&mut tools, env_list());
            }
            Capability::Network(_) => {
                push_unique(&mut tools, web_fetch());
                push_unique(&mut tools, web_search());
                push_unique(&mut tools, http_request());
                push_unique(&mut tools, http_post());
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
            Capability::AgentSpawn => {
                push_unique(&mut tools, agent_spawn());
            }
            Capability::AgentMessage => {
                push_unique(&mut tools, agent_message());
                push_unique(&mut tools, agent_list());
            }
            Capability::BrowserControl => {
                push_unique(&mut tools, browser_navigate());
                push_unique(&mut tools, browser_screenshot());
                push_unique(&mut tools, browser_click());
                push_unique(&mut tools, browser_type());
                push_unique(&mut tools, browser_content());
            }
            Capability::SourceControl => {
                push_unique(&mut tools, git_status());
                push_unique(&mut tools, git_diff());
                push_unique(&mut tools, git_log());
                push_unique(&mut tools, git_commit());
                push_unique(&mut tools, git_branch());
            }
            Capability::Container => {
                push_unique(&mut tools, docker_ps());
                push_unique(&mut tools, docker_run());
                push_unique(&mut tools, docker_build());
                push_unique(&mut tools, docker_logs());
            }
            Capability::DataManipulation => {
                push_unique(&mut tools, json_query());
                push_unique(&mut tools, json_transform());
                push_unique(&mut tools, yaml_parse());
                push_unique(&mut tools, regex_match());
                push_unique(&mut tools, regex_replace());
                push_unique(&mut tools, text_diff());
                push_unique(&mut tools, text_count());
            }
            Capability::Schedule => {
                push_unique(&mut tools, schedule_task());
                push_unique(&mut tools, schedule_list());
                push_unique(&mut tools, schedule_cancel());
            }
            Capability::CodeAnalysis => {
                push_unique(&mut tools, code_search());
                push_unique(&mut tools, code_symbols());
            }
            Capability::Archive => {
                push_unique(&mut tools, archive_create());
                push_unique(&mut tools, archive_extract());
                push_unique(&mut tools, archive_list());
            }
            Capability::Template => {
                push_unique(&mut tools, template_render());
            }
            Capability::Crypto => {
                push_unique(&mut tools, hash_compute());
                push_unique(&mut tools, hash_verify());
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
        patch_apply(),
        shell_exec(),
        web_fetch(),
        web_search(),
        memory_store(),
        memory_recall(),
        knowledge_add_entity(),
        knowledge_add_relation(),
        knowledge_query(),
        agent_spawn(),
        agent_message(),
        agent_list(),
        browser_navigate(),
        browser_screenshot(),
        browser_click(),
        browser_type(),
        browser_content(),
        // Git / Source Control
        git_status(),
        git_diff(),
        git_log(),
        git_commit(),
        git_branch(),
        // Container
        docker_ps(),
        docker_run(),
        docker_build(),
        docker_logs(),
        // HTTP
        http_request(),
        http_post(),
        // Data manipulation
        json_query(),
        json_transform(),
        yaml_parse(),
        regex_match(),
        regex_replace(),
        // Process
        process_list(),
        process_kill(),
        // Schedule
        schedule_task(),
        schedule_list(),
        schedule_cancel(),
        // Code analysis
        code_search(),
        code_symbols(),
        // Archive
        archive_create(),
        archive_extract(),
        archive_list(),
        // Template
        template_render(),
        // Crypto / Hash
        hash_compute(),
        hash_verify(),
        // Environment
        env_get(),
        env_list(),
        // Text
        text_diff(),
        text_count(),
        // File (extended)
        file_search(),
        file_info(),
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

fn web_search() -> ToolDefinition {
    ToolDefinition {
        name: "web_search".into(),
        description:
            "Search the web using DuckDuckGo and return the top results with titles and URLs."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query."
                }
            },
            "required": ["query"]
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

// ---------------------------------------------------------------------------
// Agent coordination tools
// ---------------------------------------------------------------------------

fn agent_spawn() -> ToolDefinition {
    ToolDefinition {
        name: "agent_spawn".into(),
        description: "Spawn a new fighter (AI agent). Returns the new fighter's ID. Use this to create subordinate agents that can handle specialized tasks.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A human-readable name for the new fighter."
                },
                "system_prompt": {
                    "type": "string",
                    "description": "The system prompt that shapes the new fighter's behavior and specialization."
                },
                "description": {
                    "type": "string",
                    "description": "A short description of the fighter's purpose (optional)."
                },
                "capabilities": {
                    "type": "array",
                    "description": "Capabilities to grant the new fighter (optional). Each item is a capability object.",
                    "items": {
                        "type": "object"
                    }
                }
            },
            "required": ["name", "system_prompt"]
        }),
        category: ToolCategory::Agent,
    }
}

fn agent_message() -> ToolDefinition {
    ToolDefinition {
        name: "agent_message".into(),
        description: "Send a message to another fighter by ID or name and get its response. Use this for inter-agent coordination and delegation.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "fighter_id": {
                    "type": "string",
                    "description": "The UUID of the target fighter (provide either this or 'name')."
                },
                "name": {
                    "type": "string",
                    "description": "The name of the target fighter (provide either this or 'fighter_id')."
                },
                "message": {
                    "type": "string",
                    "description": "The message to send to the target fighter."
                }
            },
            "required": ["message"]
        }),
        category: ToolCategory::Agent,
    }
}

fn agent_list() -> ToolDefinition {
    ToolDefinition {
        name: "agent_list".into(),
        description: "List all active fighters (AI agents) with their IDs, names, and status."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        category: ToolCategory::Agent,
    }
}

// ---------------------------------------------------------------------------
// Patch tools — combo move corrections
// ---------------------------------------------------------------------------

fn patch_apply() -> ToolDefinition {
    ToolDefinition {
        name: "patch_apply".into(),
        description: "Apply a unified diff patch to a file. Reads the file, validates the patch, \
                       applies it, and writes the result back. Supports standard unified diff format."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to patch (relative to working directory or absolute)."
                },
                "diff": {
                    "type": "string",
                    "description": "The unified diff text to apply to the file."
                }
            },
            "required": ["path", "diff"]
        }),
        category: ToolCategory::FileSystem,
    }
}

// ---------------------------------------------------------------------------
// Browser automation tools — ring-side scouting moves
// ---------------------------------------------------------------------------

fn browser_navigate() -> ToolDefinition {
    ToolDefinition {
        name: "browser_navigate".into(),
        description: "Navigate the browser to a URL. Opens the page and waits for it to load."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to navigate to."
                }
            },
            "required": ["url"]
        }),
        category: ToolCategory::Browser,
    }
}

fn browser_screenshot() -> ToolDefinition {
    ToolDefinition {
        name: "browser_screenshot".into(),
        description: "Take a screenshot of the current page. Returns a base64-encoded PNG image."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "full_page": {
                    "type": "boolean",
                    "description": "Capture the full scrollable page (true) or just the viewport (false). Default: false."
                }
            }
        }),
        category: ToolCategory::Browser,
    }
}

fn browser_click() -> ToolDefinition {
    ToolDefinition {
        name: "browser_click".into(),
        description: "Click an element on the page matching the given CSS selector.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector of the element to click."
                }
            },
            "required": ["selector"]
        }),
        category: ToolCategory::Browser,
    }
}

fn browser_type() -> ToolDefinition {
    ToolDefinition {
        name: "browser_type".into(),
        description: "Type text into an input element matching the given CSS selector.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector of the input element."
                },
                "text": {
                    "type": "string",
                    "description": "The text to type into the element."
                }
            },
            "required": ["selector", "text"]
        }),
        category: ToolCategory::Browser,
    }
}

fn browser_content() -> ToolDefinition {
    ToolDefinition {
        name: "browser_content".into(),
        description:
            "Get the text content of the page or a specific element. Useful for extracting readable text from a web page."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector. If omitted, returns the full page text content."
                }
            }
        }),
        category: ToolCategory::Browser,
    }
}

// ---------------------------------------------------------------------------
// Git / Source Control tools
// ---------------------------------------------------------------------------

fn git_status() -> ToolDefinition {
    ToolDefinition {
        name: "git_status".into(),
        description: "Run `git status --porcelain` in the working directory to show changed files."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        category: ToolCategory::SourceControl,
    }
}

fn git_diff() -> ToolDefinition {
    ToolDefinition {
        name: "git_diff".into(),
        description:
            "Run `git diff` to show unstaged changes. Use `staged: true` to see staged changes."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "staged": {
                    "type": "boolean",
                    "description": "If true, show staged changes (--staged). Default: false."
                },
                "path": {
                    "type": "string",
                    "description": "Optional file path to restrict the diff to."
                }
            }
        }),
        category: ToolCategory::SourceControl,
    }
}

fn git_log() -> ToolDefinition {
    ToolDefinition {
        name: "git_log".into(),
        description: "Show recent git commits with `git log --oneline`.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 10)."
                }
            }
        }),
        category: ToolCategory::SourceControl,
    }
}

fn git_commit() -> ToolDefinition {
    ToolDefinition {
        name: "git_commit".into(),
        description: "Stage files and create a git commit with the given message.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The commit message."
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files to stage before committing. If empty, commits all staged changes."
                }
            },
            "required": ["message"]
        }),
        category: ToolCategory::SourceControl,
    }
}

fn git_branch() -> ToolDefinition {
    ToolDefinition {
        name: "git_branch".into(),
        description: "List, create, or switch git branches.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "create", "switch"],
                    "description": "Action to perform: list, create, or switch. Default: list."
                },
                "name": {
                    "type": "string",
                    "description": "Branch name (required for create and switch)."
                }
            }
        }),
        category: ToolCategory::SourceControl,
    }
}

// ---------------------------------------------------------------------------
// Container tools
// ---------------------------------------------------------------------------

fn docker_ps() -> ToolDefinition {
    ToolDefinition {
        name: "docker_ps".into(),
        description: "List running Docker containers.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "all": {
                    "type": "boolean",
                    "description": "Show all containers, not just running ones. Default: false."
                }
            }
        }),
        category: ToolCategory::Container,
    }
}

fn docker_run() -> ToolDefinition {
    ToolDefinition {
        name: "docker_run".into(),
        description: "Run a Docker container from an image.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "The Docker image to run."
                },
                "command": {
                    "type": "string",
                    "description": "Optional command to run inside the container."
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables as key-value pairs."
                },
                "ports": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Port mappings (e.g. '8080:80')."
                },
                "detach": {
                    "type": "boolean",
                    "description": "Run in detached mode. Default: false."
                },
                "name": {
                    "type": "string",
                    "description": "Optional container name."
                }
            },
            "required": ["image"]
        }),
        category: ToolCategory::Container,
    }
}

fn docker_build() -> ToolDefinition {
    ToolDefinition {
        name: "docker_build".into(),
        description: "Build a Docker image from a Dockerfile.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the build context directory (default: '.')."
                },
                "tag": {
                    "type": "string",
                    "description": "Tag for the built image (e.g. 'myapp:latest')."
                },
                "dockerfile": {
                    "type": "string",
                    "description": "Path to the Dockerfile (default: 'Dockerfile')."
                }
            }
        }),
        category: ToolCategory::Container,
    }
}

fn docker_logs() -> ToolDefinition {
    ToolDefinition {
        name: "docker_logs".into(),
        description: "Get logs from a Docker container.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "container": {
                    "type": "string",
                    "description": "Container ID or name."
                },
                "tail": {
                    "type": "integer",
                    "description": "Number of lines to show from the end (default: 100)."
                }
            },
            "required": ["container"]
        }),
        category: ToolCategory::Container,
    }
}

// ---------------------------------------------------------------------------
// HTTP tools
// ---------------------------------------------------------------------------

fn http_request() -> ToolDefinition {
    ToolDefinition {
        name: "http_request".into(),
        description: "Send a full HTTP request with custom method, headers, body, and timeout."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to send the request to."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                    "description": "HTTP method. Default: GET."
                },
                "headers": {
                    "type": "object",
                    "description": "Request headers as key-value pairs."
                },
                "body": {
                    "type": "string",
                    "description": "Request body."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds (default: 30)."
                }
            },
            "required": ["url"]
        }),
        category: ToolCategory::Web,
    }
}

fn http_post() -> ToolDefinition {
    ToolDefinition {
        name: "http_post".into(),
        description: "Shorthand for an HTTP POST request with a JSON body.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to POST to."
                },
                "json": {
                    "type": "object",
                    "description": "JSON body to send."
                },
                "headers": {
                    "type": "object",
                    "description": "Additional headers."
                }
            },
            "required": ["url", "json"]
        }),
        category: ToolCategory::Web,
    }
}

// ---------------------------------------------------------------------------
// Data manipulation tools
// ---------------------------------------------------------------------------

fn json_query() -> ToolDefinition {
    ToolDefinition {
        name: "json_query".into(),
        description: "Query a JSON value using a dot-separated path (e.g. 'users.0.name'). Array indices are numeric.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "description": "The JSON data to query (object, array, or string to parse)."
                },
                "path": {
                    "type": "string",
                    "description": "Dot-separated path to query (e.g. 'a.b.0.c')."
                }
            },
            "required": ["data", "path"]
        }),
        category: ToolCategory::Data,
    }
}

fn json_transform() -> ToolDefinition {
    ToolDefinition {
        name: "json_transform".into(),
        description: "Transform JSON data: extract specific keys, rename keys, or filter an array of objects.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "description": "The JSON data to transform."
                },
                "extract": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of keys to extract from each object."
                },
                "rename": {
                    "type": "object",
                    "description": "Key rename mapping (old_name -> new_name)."
                },
                "filter_key": {
                    "type": "string",
                    "description": "Key to filter array items by."
                },
                "filter_value": {
                    "type": "string",
                    "description": "Value the filter_key must match."
                }
            },
            "required": ["data"]
        }),
        category: ToolCategory::Data,
    }
}

fn yaml_parse() -> ToolDefinition {
    ToolDefinition {
        name: "yaml_parse".into(),
        description: "Parse a YAML string and return it as JSON.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The YAML string to parse."
                }
            },
            "required": ["content"]
        }),
        category: ToolCategory::Data,
    }
}

fn regex_match() -> ToolDefinition {
    ToolDefinition {
        name: "regex_match".into(),
        description: "Match a regex pattern against text and return all captures.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern."
                },
                "text": {
                    "type": "string",
                    "description": "The text to match against."
                },
                "global": {
                    "type": "boolean",
                    "description": "Find all matches (true) or just the first (false). Default: false."
                }
            },
            "required": ["pattern", "text"]
        }),
        category: ToolCategory::Data,
    }
}

fn regex_replace() -> ToolDefinition {
    ToolDefinition {
        name: "regex_replace".into(),
        description: "Find and replace text using a regex pattern. Supports capture group references ($1, $2, etc.) in the replacement.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to find."
                },
                "replacement": {
                    "type": "string",
                    "description": "The replacement string (supports $1, $2, etc. for captures)."
                },
                "text": {
                    "type": "string",
                    "description": "The text to perform replacement on."
                }
            },
            "required": ["pattern", "replacement", "text"]
        }),
        category: ToolCategory::Data,
    }
}

// ---------------------------------------------------------------------------
// Process tools
// ---------------------------------------------------------------------------

fn process_list() -> ToolDefinition {
    ToolDefinition {
        name: "process_list".into(),
        description: "List running processes with PID, name, and CPU/memory usage.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional filter string to match process names."
                }
            }
        }),
        category: ToolCategory::Shell,
    }
}

fn process_kill() -> ToolDefinition {
    ToolDefinition {
        name: "process_kill".into(),
        description: "Kill a process by PID.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "The process ID to kill."
                },
                "signal": {
                    "type": "string",
                    "description": "Signal to send (e.g. 'TERM', 'KILL'). Default: 'TERM'."
                }
            },
            "required": ["pid"]
        }),
        category: ToolCategory::Shell,
    }
}

// ---------------------------------------------------------------------------
// Schedule tools
// ---------------------------------------------------------------------------

fn schedule_task() -> ToolDefinition {
    ToolDefinition {
        name: "schedule_task".into(),
        description: "Schedule a one-shot or recurring task. Returns a task ID.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable name for the task."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to execute when the task fires."
                },
                "delay_secs": {
                    "type": "integer",
                    "description": "Delay in seconds before first execution."
                },
                "interval_secs": {
                    "type": "integer",
                    "description": "Interval in seconds for recurring execution. If omitted, the task runs once."
                }
            },
            "required": ["name", "command", "delay_secs"]
        }),
        category: ToolCategory::Schedule,
    }
}

fn schedule_list() -> ToolDefinition {
    ToolDefinition {
        name: "schedule_list".into(),
        description: "List all scheduled tasks with their IDs, names, and status.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        category: ToolCategory::Schedule,
    }
}

fn schedule_cancel() -> ToolDefinition {
    ToolDefinition {
        name: "schedule_cancel".into(),
        description: "Cancel a scheduled task by its ID.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The UUID of the task to cancel."
                }
            },
            "required": ["task_id"]
        }),
        category: ToolCategory::Schedule,
    }
}

// ---------------------------------------------------------------------------
// Code analysis tools
// ---------------------------------------------------------------------------

fn code_search() -> ToolDefinition {
    ToolDefinition {
        name: "code_search".into(),
        description: "Search for text or a regex pattern in files recursively under a directory. Returns matching lines with file paths and line numbers.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search in (default: working directory)."
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. '*.rs', '*.py')."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return (default: 50)."
                }
            },
            "required": ["pattern"]
        }),
        category: ToolCategory::CodeAnalysis,
    }
}

fn code_symbols() -> ToolDefinition {
    ToolDefinition {
        name: "code_symbols".into(),
        description: "Extract function, struct, class, and method definitions from a source file using regex-based heuristics.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the source file to analyze."
                }
            },
            "required": ["path"]
        }),
        category: ToolCategory::CodeAnalysis,
    }
}

// ---------------------------------------------------------------------------
// Archive tools
// ---------------------------------------------------------------------------

fn archive_create() -> ToolDefinition {
    ToolDefinition {
        name: "archive_create".into(),
        description: "Create a tar.gz archive from a list of file or directory paths.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "output_path": {
                    "type": "string",
                    "description": "Path for the output .tar.gz archive file."
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file or directory paths to include in the archive."
                }
            },
            "required": ["output_path", "paths"]
        }),
        category: ToolCategory::Archive,
    }
}

fn archive_extract() -> ToolDefinition {
    ToolDefinition {
        name: "archive_extract".into(),
        description: "Extract a tar.gz archive to a destination directory.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "archive_path": {
                    "type": "string",
                    "description": "Path to the .tar.gz archive to extract."
                },
                "destination": {
                    "type": "string",
                    "description": "Directory to extract the archive into."
                }
            },
            "required": ["archive_path", "destination"]
        }),
        category: ToolCategory::Archive,
    }
}

fn archive_list() -> ToolDefinition {
    ToolDefinition {
        name: "archive_list".into(),
        description: "List the contents of a tar.gz archive without extracting.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "archive_path": {
                    "type": "string",
                    "description": "Path to the .tar.gz archive to list."
                }
            },
            "required": ["archive_path"]
        }),
        category: ToolCategory::Archive,
    }
}

// ---------------------------------------------------------------------------
// Template tools
// ---------------------------------------------------------------------------

fn template_render() -> ToolDefinition {
    ToolDefinition {
        name: "template_render".into(),
        description: "Render a Handlebars-style template by substituting {{variable}} placeholders with provided values.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "template": {
                    "type": "string",
                    "description": "The template string containing {{variable}} placeholders."
                },
                "variables": {
                    "type": "object",
                    "description": "Key-value pairs mapping variable names to their values."
                }
            },
            "required": ["template", "variables"]
        }),
        category: ToolCategory::Template,
    }
}

// ---------------------------------------------------------------------------
// Crypto / Hash tools
// ---------------------------------------------------------------------------

fn hash_compute() -> ToolDefinition {
    ToolDefinition {
        name: "hash_compute".into(),
        description: "Compute a cryptographic hash (SHA-256, SHA-512, or MD5) of a string or file."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "algorithm": {
                    "type": "string",
                    "enum": ["sha256", "sha512", "md5"],
                    "description": "Hash algorithm to use. Default: sha256."
                },
                "input": {
                    "type": "string",
                    "description": "The string to hash (provide either this or 'file')."
                },
                "file": {
                    "type": "string",
                    "description": "Path to a file to hash (provide either this or 'input')."
                }
            }
        }),
        category: ToolCategory::Crypto,
    }
}

fn hash_verify() -> ToolDefinition {
    ToolDefinition {
        name: "hash_verify".into(),
        description: "Verify that a hash matches an expected value.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "algorithm": {
                    "type": "string",
                    "enum": ["sha256", "sha512", "md5"],
                    "description": "Hash algorithm to use. Default: sha256."
                },
                "input": {
                    "type": "string",
                    "description": "The string to hash (provide either this or 'file')."
                },
                "file": {
                    "type": "string",
                    "description": "Path to a file to hash (provide either this or 'input')."
                },
                "expected": {
                    "type": "string",
                    "description": "The expected hex-encoded hash value to compare against."
                }
            },
            "required": ["expected"]
        }),
        category: ToolCategory::Crypto,
    }
}

// ---------------------------------------------------------------------------
// Environment tools
// ---------------------------------------------------------------------------

fn env_get() -> ToolDefinition {
    ToolDefinition {
        name: "env_get".into(),
        description: "Get the value of an environment variable.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The environment variable name."
                }
            },
            "required": ["name"]
        }),
        category: ToolCategory::Shell,
    }
}

fn env_list() -> ToolDefinition {
    ToolDefinition {
        name: "env_list".into(),
        description: "List all environment variables, with optional prefix filter.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prefix": {
                    "type": "string",
                    "description": "Optional prefix to filter environment variable names by."
                }
            }
        }),
        category: ToolCategory::Shell,
    }
}

// ---------------------------------------------------------------------------
// Text tools
// ---------------------------------------------------------------------------

fn text_diff() -> ToolDefinition {
    ToolDefinition {
        name: "text_diff".into(),
        description: "Compute a unified diff between two text strings.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "old_text": {
                    "type": "string",
                    "description": "The original text."
                },
                "new_text": {
                    "type": "string",
                    "description": "The modified text."
                },
                "label": {
                    "type": "string",
                    "description": "Optional label for the diff output (default: 'a' / 'b')."
                }
            },
            "required": ["old_text", "new_text"]
        }),
        category: ToolCategory::Data,
    }
}

fn text_count() -> ToolDefinition {
    ToolDefinition {
        name: "text_count".into(),
        description: "Count lines, words, and characters in text.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to count."
                }
            },
            "required": ["text"]
        }),
        category: ToolCategory::Data,
    }
}

// ---------------------------------------------------------------------------
// File tools (extended)
// ---------------------------------------------------------------------------

fn file_search() -> ToolDefinition {
    ToolDefinition {
        name: "file_search".into(),
        description: "Search for files by name pattern (glob) recursively under a directory."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match file names (e.g. '*.rs', 'Cargo.*')."
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search in (default: working directory)."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)."
                }
            },
            "required": ["pattern"]
        }),
        category: ToolCategory::FileSystem,
    }
}

fn file_info() -> ToolDefinition {
    ToolDefinition {
        name: "file_info".into(),
        description: "Get file metadata: size, modified time, permissions, and type.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file or directory to inspect."
                }
            },
            "required": ["path"]
        }),
        category: ToolCategory::FileSystem,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_tool_definitions_correct() {
        let nav = browser_navigate();
        assert_eq!(nav.name, "browser_navigate");
        assert_eq!(nav.category, ToolCategory::Browser);
        assert!(nav.input_schema["required"]
            .as_array()
            .expect("required should be array")
            .iter()
            .any(|v| v == "url"));

        let ss = browser_screenshot();
        assert_eq!(ss.name, "browser_screenshot");
        assert_eq!(ss.category, ToolCategory::Browser);

        let click = browser_click();
        assert_eq!(click.name, "browser_click");
        assert!(click.input_schema["required"]
            .as_array()
            .expect("required should be array")
            .iter()
            .any(|v| v == "selector"));

        let typ = browser_type();
        assert_eq!(typ.name, "browser_type");
        let required = typ.input_schema["required"]
            .as_array()
            .expect("required should be array");
        assert!(required.iter().any(|v| v == "selector"));
        assert!(required.iter().any(|v| v == "text"));

        let content = browser_content();
        assert_eq!(content.name, "browser_content");
        assert_eq!(content.category, ToolCategory::Browser);
    }

    #[test]
    fn test_browser_tools_require_browser_control_capability() {
        let caps = vec![Capability::BrowserControl];
        let tools = tools_for_capabilities(&caps);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"browser_navigate"),
            "missing browser_navigate"
        );
        assert!(
            names.contains(&"browser_screenshot"),
            "missing browser_screenshot"
        );
        assert!(names.contains(&"browser_click"), "missing browser_click");
        assert!(names.contains(&"browser_type"), "missing browser_type");
        assert!(
            names.contains(&"browser_content"),
            "missing browser_content"
        );
    }

    #[test]
    fn test_browser_tools_absent_without_capability() {
        let caps = vec![Capability::Memory];
        let tools = tools_for_capabilities(&caps);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("browser_")));
    }

    #[test]
    fn test_all_tools_includes_browser() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_type"));
        assert!(names.contains(&"browser_content"));
    }
}
