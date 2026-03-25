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
            Capability::A2ADelegate => {
                push_unique(&mut tools, a2a_delegate());
            }
            Capability::PluginInvoke => {
                push_unique(&mut tools, wasm_invoke());
            }
            Capability::ChannelNotify => {
                push_unique(&mut tools, channel_notify());
            }
            Capability::SelfConfig => {
                push_unique(&mut tools, heartbeat_add());
                push_unique(&mut tools, heartbeat_list());
                push_unique(&mut tools, heartbeat_remove());
                push_unique(&mut tools, creed_view());
                push_unique(&mut tools, skill_list());
                push_unique(&mut tools, skill_recommend());
            }
            Capability::SystemAutomation => {
                push_unique(&mut tools, sys_screenshot());
            }
            Capability::UiAutomation(_) => {
                push_unique(&mut tools, ui_screenshot());
                push_unique(&mut tools, ui_find_elements());
                push_unique(&mut tools, ui_click());
                push_unique(&mut tools, ui_type_text());
                push_unique(&mut tools, ui_list_windows());
                push_unique(&mut tools, ui_read_attribute());
            }
            Capability::AppIntegration(_) => {
                push_unique(&mut tools, app_ocr());
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
        // A2A delegation
        a2a_delegate(),
        // WASM Plugin
        wasm_invoke(),
        // Channel notification
        channel_notify(),
        // Self-configuration
        heartbeat_add(),
        heartbeat_list(),
        heartbeat_remove(),
        creed_view(),
        skill_list(),
        skill_recommend(),
        // Desktop automation
        sys_screenshot(),
        ui_screenshot(),
        app_ocr(),
        ui_find_elements(),
        ui_click(),
        ui_type_text(),
        ui_list_windows(),
        ui_read_attribute(),
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

fn a2a_delegate() -> ToolDefinition {
    ToolDefinition {
        name: "a2a_delegate".into(),
        description: "Delegate a task to a remote A2A agent. Discovers the agent, sends the task, \
                      polls for completion, and returns the result."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_url": {
                    "type": "string",
                    "description": "Base URL of the remote A2A agent (e.g. 'https://agent.example.com')."
                },
                "prompt": {
                    "type": "string",
                    "description": "The task description / prompt to send to the remote agent."
                },
                "context": {
                    "type": "object",
                    "description": "Optional additional context as key-value pairs."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum time to wait for the task to complete (default: 60)."
                }
            },
            "required": ["agent_url", "prompt"]
        }),
        category: ToolCategory::Agent,
    }
}

fn wasm_invoke() -> ToolDefinition {
    ToolDefinition {
        name: "wasm_invoke".into(),
        description: "Invoke a function on a loaded WASM plugin (imported technique). \
                      Executes the named function within the plugin's sandboxed WASM runtime \
                      and returns the result."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "plugin": {
                    "type": "string",
                    "description": "Name of the loaded WASM plugin to invoke."
                },
                "function": {
                    "type": "string",
                    "description": "Name of the exported function to call within the plugin."
                },
                "input": {
                    "type": "object",
                    "description": "Input arguments to pass to the plugin function (optional)."
                }
            },
            "required": ["plugin", "function"]
        }),
        category: ToolCategory::Plugin,
    }
}

fn channel_notify() -> ToolDefinition {
    ToolDefinition {
        name: "channel_notify".into(),
        description: "Send a proactive message to an external channel (Telegram, Slack, Discord, \
                      etc.). Use this to push notifications, briefings, alerts, and heartbeat \
                      results to connected messaging platforms."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "The channel adapter name (e.g., \"telegram\", \"discord\", \"slack\")."
                },
                "chat_id": {
                    "type": "string",
                    "description": "The channel/conversation ID to send the message to."
                },
                "message": {
                    "type": "string",
                    "description": "The text message to send. Keep it concise and actionable."
                }
            },
            "required": ["channel", "chat_id", "message"]
        }),
        category: ToolCategory::Channel,
    }
}

// ---------------------------------------------------------------------------
// Self-Configuration Tools
// ---------------------------------------------------------------------------

fn heartbeat_add() -> ToolDefinition {
    ToolDefinition {
        name: "heartbeat_add".into(),
        description: "Add a proactive heartbeat task to your creed. Heartbeat tasks fire on a \
                      cadence (every_bout, on_wake, hourly, daily) and remind you to perform \
                      recurring actions like morning briefings, health checks, or summaries."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "What to do when the heartbeat fires (e.g., \"Morning briefing: summarize calendar and emails\")."
                },
                "cadence": {
                    "type": "string",
                    "enum": ["every_bout", "on_wake", "hourly", "daily"],
                    "description": "How often: every_bout (every conversation), on_wake (first bout after restart), hourly, daily."
                }
            },
            "required": ["task", "cadence"]
        }),
        category: ToolCategory::Agent,
    }
}

fn heartbeat_list() -> ToolDefinition {
    ToolDefinition {
        name: "heartbeat_list".into(),
        description: "List all heartbeat tasks in your creed. Shows task description, cadence, \
                      active status, execution count, and last checked time."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: ToolCategory::Agent,
    }
}

fn heartbeat_remove() -> ToolDefinition {
    ToolDefinition {
        name: "heartbeat_remove".into(),
        description: "Remove a heartbeat task from your creed by its index (0-based). Use \
                      heartbeat_list first to see the indices."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "index": {
                    "type": "integer",
                    "description": "The 0-based index of the heartbeat task to remove."
                }
            },
            "required": ["index"]
        }),
        category: ToolCategory::Agent,
    }
}

fn creed_view() -> ToolDefinition {
    ToolDefinition {
        name: "creed_view".into(),
        description: "View your current creed — identity, personality traits, directives, \
                      learned behaviors, relationships, heartbeat tasks, and stats. Use this \
                      to understand who you are and what you're configured to do."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: ToolCategory::Agent,
    }
}

fn skill_list() -> ToolDefinition {
    ToolDefinition {
        name: "skill_list".into(),
        description: "List available skill packs that can be installed. Skill packs bundle MCP \
                      server configurations with prompts and tools. Available packs: productivity \
                      (calendar/email), developer (GitHub), research (web tools), files (filesystem)."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: ToolCategory::Agent,
    }
}

fn skill_recommend() -> ToolDefinition {
    ToolDefinition {
        name: "skill_recommend".into(),
        description: "Recommend a skill pack to the user based on what they need. Looks up the \
                      pack details (what it provides, required setup, install command) and returns \
                      a recommendation the user can act on. Use this when the user asks for \
                      capabilities you don't currently have (e.g., calendar, email, GitHub)."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pack_name": {
                    "type": "string",
                    "description": "The skill pack name to recommend (e.g., \"productivity\", \"developer\", \"research\", \"files\")."
                }
            },
            "required": ["pack_name"]
        }),
        category: ToolCategory::Agent,
    }
}

// ---------------------------------------------------------------------------
// Desktop automation tool definitions
// ---------------------------------------------------------------------------

fn sys_screenshot() -> ToolDefinition {
    ToolDefinition {
        name: "sys_screenshot".into(),
        description: "Capture a screenshot of the full screen or a specific window. Returns a base64-encoded PNG image that the vision model can read. Use this to see what's currently on screen.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "window": {
                    "type": "string",
                    "description": "Optional window title to capture. If omitted, captures the full screen."
                }
            }
        }),
        category: ToolCategory::SystemAutomation,
    }
}

fn ui_screenshot() -> ToolDefinition {
    ToolDefinition {
        name: "ui_screenshot".into(),
        description: "Capture a screenshot of a specific UI region by element ID or bounds. More targeted than sys_screenshot for inspecting specific parts of the screen.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "element_id": {
                    "type": "string",
                    "description": "Element ID from ui_find_elements (e.g. \"Safari:3\"). Captures the region of that element."
                },
                "bounds": {
                    "type": "object",
                    "description": "Explicit bounds to capture: {x, y, width, height} in pixels.",
                    "properties": {
                        "x": {"type": "integer"},
                        "y": {"type": "integer"},
                        "width": {"type": "integer"},
                        "height": {"type": "integer"}
                    },
                    "required": ["x", "y", "width", "height"]
                }
            }
        }),
        category: ToolCategory::UiAutomation,
    }
}

fn app_ocr() -> ToolDefinition {
    ToolDefinition {
        name: "app_ocr".into(),
        description: "Extract text from an app window using OCR (optical character recognition). Returns plain text — cheaper than a screenshot + vision model for text-heavy content. Use this first for reading text, fall back to sys_screenshot for visual/spatial understanding.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "app": {
                    "type": "string",
                    "description": "Name of the application to OCR (e.g. \"Messages\", \"Safari\")."
                }
            },
            "required": ["app"]
        }),
        category: ToolCategory::AppIntegration,
    }
}

fn ui_find_elements() -> ToolDefinition {
    ToolDefinition {
        name: "ui_find_elements".into(),
        description: "Query the accessibility tree of an app to find UI elements (buttons, text fields, rows, etc.). Returns structured element IDs that can be used with ui_click, ui_type_text, and ui_read_attribute. Re-query if the app state changes, as element IDs are session-ephemeral.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "app": {
                    "type": "string",
                    "description": "Name of the application to query (e.g. \"Messages\", \"Safari\")."
                },
                "role": {
                    "type": "string",
                    "description": "Optional: filter by accessibility role (e.g. \"button\", \"text field\", \"row\", \"menu item\")."
                },
                "label": {
                    "type": "string",
                    "description": "Optional: filter by accessibility label (substring match)."
                },
                "value": {
                    "type": "string",
                    "description": "Optional: filter by current value (substring match)."
                }
            },
            "required": ["app"]
        }),
        category: ToolCategory::UiAutomation,
    }
}

fn ui_click() -> ToolDefinition {
    ToolDefinition {
        name: "ui_click".into(),
        description: "Click a UI element by its element ID (from ui_find_elements). Safe, validated accessibility click — not a raw coordinate click.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "element_id": {
                    "type": "string",
                    "description": "Element ID from ui_find_elements (e.g. \"Messages:0\")."
                }
            },
            "required": ["element_id"]
        }),
        category: ToolCategory::UiAutomation,
    }
}

fn ui_type_text() -> ToolDefinition {
    ToolDefinition {
        name: "ui_type_text".into(),
        description: "Type text into a UI element by its element ID (from ui_find_elements). Sets the value of a text field or input element.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "element_id": {
                    "type": "string",
                    "description": "Element ID of the text field (e.g. \"Messages:2\")."
                },
                "text": {
                    "type": "string",
                    "description": "The text to type into the element."
                }
            },
            "required": ["element_id", "text"]
        }),
        category: ToolCategory::UiAutomation,
    }
}

fn ui_list_windows() -> ToolDefinition {
    ToolDefinition {
        name: "ui_list_windows".into(),
        description: "List all visible windows with their titles and owning apps. Use this to discover what's on screen before taking a screenshot or interacting with specific apps.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        category: ToolCategory::UiAutomation,
    }
}

fn ui_read_attribute() -> ToolDefinition {
    ToolDefinition {
        name: "ui_read_attribute".into(),
        description: "Read an accessibility attribute (value, enabled, focused, etc.) from a UI element. Useful for checking element state without a screenshot.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "element_id": {
                    "type": "string",
                    "description": "Element ID from ui_find_elements (e.g. \"Safari:3\")."
                },
                "attribute": {
                    "type": "string",
                    "description": "The attribute to read. Allowed: value, name, role, role description, title, description, enabled, focused, position, size, selected, help, subrole, identifier, minimum value, maximum value, orientation, placeholder value."
                }
            },
            "required": ["element_id", "attribute"]
        }),
        category: ToolCategory::UiAutomation,
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
        assert!(
            nav.input_schema["required"]
                .as_array()
                .expect("required should be array")
                .iter()
                .any(|v| v == "url")
        );

        let ss = browser_screenshot();
        assert_eq!(ss.name, "browser_screenshot");
        assert_eq!(ss.category, ToolCategory::Browser);

        let click = browser_click();
        assert_eq!(click.name, "browser_click");
        assert!(
            click.input_schema["required"]
                .as_array()
                .expect("required should be array")
                .iter()
                .any(|v| v == "selector")
        );

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

    // -----------------------------------------------------------------------
    // Tool definition correctness tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_file_read_definition() {
        let t = file_read();
        assert_eq!(t.name, "file_read");
        assert_eq!(t.category, ToolCategory::FileSystem);
        assert_eq!(t.input_schema["type"], "object");
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }

    #[test]
    fn test_file_write_definition() {
        let t = file_write();
        assert_eq!(t.name, "file_write");
        assert_eq!(t.category, ToolCategory::FileSystem);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
        assert!(required.iter().any(|v| v == "content"));
    }

    #[test]
    fn test_file_list_definition() {
        let t = file_list();
        assert_eq!(t.name, "file_list");
        assert_eq!(t.category, ToolCategory::FileSystem);
        assert_eq!(t.input_schema["type"], "object");
    }

    #[test]
    fn test_file_search_definition() {
        let t = file_search();
        assert_eq!(t.name, "file_search");
        assert_eq!(t.category, ToolCategory::FileSystem);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pattern"));
    }

    #[test]
    fn test_file_info_definition() {
        let t = file_info();
        assert_eq!(t.name, "file_info");
        assert_eq!(t.category, ToolCategory::FileSystem);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }

    #[test]
    fn test_patch_apply_definition() {
        let t = patch_apply();
        assert_eq!(t.name, "patch_apply");
        assert_eq!(t.category, ToolCategory::FileSystem);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
        assert!(required.iter().any(|v| v == "diff"));
    }

    #[test]
    fn test_shell_exec_definition() {
        let t = shell_exec();
        assert_eq!(t.name, "shell_exec");
        assert_eq!(t.category, ToolCategory::Shell);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "command"));
    }

    #[test]
    fn test_web_fetch_definition() {
        let t = web_fetch();
        assert_eq!(t.name, "web_fetch");
        assert_eq!(t.category, ToolCategory::Web);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "url"));
    }

    #[test]
    fn test_web_search_definition() {
        let t = web_search();
        assert_eq!(t.name, "web_search");
        assert_eq!(t.category, ToolCategory::Web);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn test_memory_store_definition() {
        let t = memory_store();
        assert_eq!(t.name, "memory_store");
        assert_eq!(t.category, ToolCategory::Memory);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "key"));
        assert!(required.iter().any(|v| v == "value"));
    }

    #[test]
    fn test_memory_recall_definition() {
        let t = memory_recall();
        assert_eq!(t.name, "memory_recall");
        assert_eq!(t.category, ToolCategory::Memory);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn test_knowledge_tools_definitions() {
        let ae = knowledge_add_entity();
        assert_eq!(ae.name, "knowledge_add_entity");
        assert_eq!(ae.category, ToolCategory::Knowledge);
        let required = ae.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
        assert!(required.iter().any(|v| v == "entity_type"));

        let ar = knowledge_add_relation();
        assert_eq!(ar.name, "knowledge_add_relation");
        let required = ar.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "from"));
        assert!(required.iter().any(|v| v == "relation"));
        assert!(required.iter().any(|v| v == "to"));

        let kq = knowledge_query();
        assert_eq!(kq.name, "knowledge_query");
        let required = kq.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn test_agent_tools_definitions() {
        let spawn = agent_spawn();
        assert_eq!(spawn.name, "agent_spawn");
        assert_eq!(spawn.category, ToolCategory::Agent);
        let required = spawn.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
        assert!(required.iter().any(|v| v == "system_prompt"));

        let msg = agent_message();
        assert_eq!(msg.name, "agent_message");
        assert_eq!(msg.category, ToolCategory::Agent);
        let required = msg.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "message"));

        let list = agent_list();
        assert_eq!(list.name, "agent_list");
        assert_eq!(list.category, ToolCategory::Agent);
    }

    #[test]
    fn test_git_tools_definitions() {
        let status = git_status();
        assert_eq!(status.name, "git_status");
        assert_eq!(status.category, ToolCategory::SourceControl);

        let diff = git_diff();
        assert_eq!(diff.name, "git_diff");

        let log = git_log();
        assert_eq!(log.name, "git_log");

        let commit = git_commit();
        assert_eq!(commit.name, "git_commit");
        let required = commit.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "message"));

        let branch = git_branch();
        assert_eq!(branch.name, "git_branch");
    }

    #[test]
    fn test_docker_tools_definitions() {
        let ps = docker_ps();
        assert_eq!(ps.name, "docker_ps");
        assert_eq!(ps.category, ToolCategory::Container);

        let run = docker_run();
        assert_eq!(run.name, "docker_run");
        let required = run.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "image"));

        let build = docker_build();
        assert_eq!(build.name, "docker_build");

        let logs = docker_logs();
        assert_eq!(logs.name, "docker_logs");
        let required = logs.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "container"));
    }

    #[test]
    fn test_http_tools_definitions() {
        let req = http_request();
        assert_eq!(req.name, "http_request");
        assert_eq!(req.category, ToolCategory::Web);
        let required = req.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "url"));

        let post = http_post();
        assert_eq!(post.name, "http_post");
        let required = post.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "url"));
        assert!(required.iter().any(|v| v == "json"));
    }

    #[test]
    fn test_data_tools_definitions() {
        let jq = json_query();
        assert_eq!(jq.name, "json_query");
        assert_eq!(jq.category, ToolCategory::Data);
        let required = jq.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "data"));
        assert!(required.iter().any(|v| v == "path"));

        let jt = json_transform();
        assert_eq!(jt.name, "json_transform");
        let required = jt.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "data"));

        let yp = yaml_parse();
        assert_eq!(yp.name, "yaml_parse");
        let required = yp.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "content"));

        let rm = regex_match();
        assert_eq!(rm.name, "regex_match");
        let required = rm.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pattern"));
        assert!(required.iter().any(|v| v == "text"));

        let rr = regex_replace();
        assert_eq!(rr.name, "regex_replace");
        let required = rr.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pattern"));
        assert!(required.iter().any(|v| v == "replacement"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn test_process_tools_definitions() {
        let pl = process_list();
        assert_eq!(pl.name, "process_list");
        assert_eq!(pl.category, ToolCategory::Shell);

        let pk = process_kill();
        assert_eq!(pk.name, "process_kill");
        let required = pk.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pid"));
    }

    #[test]
    fn test_schedule_tools_definitions() {
        let st = schedule_task();
        assert_eq!(st.name, "schedule_task");
        assert_eq!(st.category, ToolCategory::Schedule);
        let required = st.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
        assert!(required.iter().any(|v| v == "command"));
        assert!(required.iter().any(|v| v == "delay_secs"));

        let sl = schedule_list();
        assert_eq!(sl.name, "schedule_list");

        let sc = schedule_cancel();
        assert_eq!(sc.name, "schedule_cancel");
        let required = sc.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "task_id"));
    }

    #[test]
    fn test_code_analysis_tools_definitions() {
        let cs = code_search();
        assert_eq!(cs.name, "code_search");
        assert_eq!(cs.category, ToolCategory::CodeAnalysis);
        let required = cs.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pattern"));

        let sym = code_symbols();
        assert_eq!(sym.name, "code_symbols");
        let required = sym.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }

    #[test]
    fn test_archive_tools_definitions() {
        let ac = archive_create();
        assert_eq!(ac.name, "archive_create");
        assert_eq!(ac.category, ToolCategory::Archive);
        let required = ac.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "output_path"));
        assert!(required.iter().any(|v| v == "paths"));

        let ae = archive_extract();
        assert_eq!(ae.name, "archive_extract");
        let required = ae.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "archive_path"));
        assert!(required.iter().any(|v| v == "destination"));

        let al = archive_list();
        assert_eq!(al.name, "archive_list");
        let required = al.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "archive_path"));
    }

    #[test]
    fn test_template_render_definition() {
        let t = template_render();
        assert_eq!(t.name, "template_render");
        assert_eq!(t.category, ToolCategory::Template);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "template"));
        assert!(required.iter().any(|v| v == "variables"));
    }

    #[test]
    fn test_crypto_tools_definitions() {
        let hc = hash_compute();
        assert_eq!(hc.name, "hash_compute");
        assert_eq!(hc.category, ToolCategory::Crypto);

        let hv = hash_verify();
        assert_eq!(hv.name, "hash_verify");
        let required = hv.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "expected"));
    }

    #[test]
    fn test_env_tools_definitions() {
        let eg = env_get();
        assert_eq!(eg.name, "env_get");
        assert_eq!(eg.category, ToolCategory::Shell);
        let required = eg.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));

        let el = env_list();
        assert_eq!(el.name, "env_list");
    }

    #[test]
    fn test_text_tools_definitions() {
        let td = text_diff();
        assert_eq!(td.name, "text_diff");
        assert_eq!(td.category, ToolCategory::Data);
        let required = td.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "old_text"));
        assert!(required.iter().any(|v| v == "new_text"));

        let tc = text_count();
        assert_eq!(tc.name, "text_count");
        let required = tc.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "text"));
    }

    // -----------------------------------------------------------------------
    // tools_for_capabilities tests per capability variant
    // -----------------------------------------------------------------------

    #[test]
    fn test_tools_for_file_read_capability() {
        let caps = vec![Capability::FileRead("**".into())];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_list"));
        assert!(names.contains(&"file_search"));
        assert!(names.contains(&"file_info"));
        assert!(!names.contains(&"file_write"));
    }

    #[test]
    fn test_tools_for_file_write_capability() {
        let caps = vec![Capability::FileWrite("**".into())];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"file_write"));
        assert!(names.contains(&"patch_apply"));
        assert!(!names.contains(&"file_read"));
    }

    #[test]
    fn test_tools_for_shell_exec_capability() {
        let caps = vec![Capability::ShellExec("*".into())];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"shell_exec"));
        assert!(names.contains(&"process_list"));
        assert!(names.contains(&"process_kill"));
        assert!(names.contains(&"env_get"));
        assert!(names.contains(&"env_list"));
    }

    #[test]
    fn test_tools_for_network_capability() {
        let caps = vec![Capability::Network("*".into())];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"http_request"));
        assert!(names.contains(&"http_post"));
    }

    #[test]
    fn test_tools_for_memory_capability() {
        let caps = vec![Capability::Memory];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"memory_store"));
        assert!(names.contains(&"memory_recall"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_tools_for_knowledge_graph_capability() {
        let caps = vec![Capability::KnowledgeGraph];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"knowledge_add_entity"));
        assert!(names.contains(&"knowledge_add_relation"));
        assert!(names.contains(&"knowledge_query"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_tools_for_agent_spawn_capability() {
        let caps = vec![Capability::AgentSpawn];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"agent_spawn"));
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn test_tools_for_agent_message_capability() {
        let caps = vec![Capability::AgentMessage];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"agent_message"));
        assert!(names.contains(&"agent_list"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_tools_for_source_control_capability() {
        let caps = vec![Capability::SourceControl];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_diff"));
        assert!(names.contains(&"git_log"));
        assert!(names.contains(&"git_commit"));
        assert!(names.contains(&"git_branch"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn test_tools_for_container_capability() {
        let caps = vec![Capability::Container];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"docker_ps"));
        assert!(names.contains(&"docker_run"));
        assert!(names.contains(&"docker_build"));
        assert!(names.contains(&"docker_logs"));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_tools_for_data_manipulation_capability() {
        let caps = vec![Capability::DataManipulation];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"json_query"));
        assert!(names.contains(&"json_transform"));
        assert!(names.contains(&"yaml_parse"));
        assert!(names.contains(&"regex_match"));
        assert!(names.contains(&"regex_replace"));
        assert!(names.contains(&"text_diff"));
        assert!(names.contains(&"text_count"));
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn test_tools_for_schedule_capability() {
        let caps = vec![Capability::Schedule];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"schedule_task"));
        assert!(names.contains(&"schedule_list"));
        assert!(names.contains(&"schedule_cancel"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_tools_for_code_analysis_capability() {
        let caps = vec![Capability::CodeAnalysis];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"code_search"));
        assert!(names.contains(&"code_symbols"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_tools_for_archive_capability() {
        let caps = vec![Capability::Archive];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"archive_create"));
        assert!(names.contains(&"archive_extract"));
        assert!(names.contains(&"archive_list"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_tools_for_template_capability() {
        let caps = vec![Capability::Template];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"template_render"));
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn test_tools_for_crypto_capability() {
        let caps = vec![Capability::Crypto];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"hash_compute"));
        assert!(names.contains(&"hash_verify"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_tools_for_empty_capabilities() {
        let caps: Vec<Capability> = vec![];
        let tools = tools_for_capabilities(&caps);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_tools_for_event_publish_returns_empty() {
        // EventPublish is in the _ => {} catch-all
        let caps = vec![Capability::EventPublish];
        let tools = tools_for_capabilities(&caps);
        assert!(tools.is_empty());
    }

    // -----------------------------------------------------------------------
    // push_unique dedup tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_push_unique_dedup() {
        let mut tools = Vec::new();
        push_unique(&mut tools, file_read());
        push_unique(&mut tools, file_read());
        push_unique(&mut tools, file_read());
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn test_push_unique_different_tools() {
        let mut tools = Vec::new();
        push_unique(&mut tools, file_read());
        push_unique(&mut tools, file_write());
        push_unique(&mut tools, shell_exec());
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn test_tools_for_multiple_capabilities_dedup() {
        // FileRead + FileRead should not double-add tools
        let caps = vec![
            Capability::FileRead("src/**".into()),
            Capability::FileRead("tests/**".into()),
        ];
        let tools = tools_for_capabilities(&caps);
        let file_read_count = tools.iter().filter(|t| t.name == "file_read").count();
        assert_eq!(file_read_count, 1);
    }

    // -----------------------------------------------------------------------
    // all_tools tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_tools_count() {
        let tools = all_tools();
        // Count the items in the vec literal in all_tools()
        assert!(
            tools.len() >= 50,
            "expected at least 50 tools, got {}",
            tools.len()
        );
    }

    #[test]
    fn test_all_tools_unique_names() {
        let tools = all_tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "all_tools has duplicate names");
    }

    #[test]
    fn test_all_tools_valid_schemas() {
        let tools = all_tools();
        for tool in &tools {
            assert_eq!(
                tool.input_schema["type"], "object",
                "tool {} has non-object schema",
                tool.name
            );
            assert!(!tool.name.is_empty(), "tool has empty name");
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
        }
    }

    #[test]
    fn test_a2a_delegate_tool_definition() {
        let tool = a2a_delegate();
        assert_eq!(tool.name, "a2a_delegate");
        assert_eq!(tool.category, ToolCategory::Agent);
        let required = tool.input_schema["required"]
            .as_array()
            .expect("required should be array");
        assert!(required.iter().any(|v| v == "agent_url"));
        assert!(required.iter().any(|v| v == "prompt"));
    }

    #[test]
    fn test_tools_for_a2a_delegate_capability() {
        let caps = vec![Capability::A2ADelegate];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a2a_delegate"));
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn test_all_tools_includes_a2a_delegate() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a2a_delegate"));
    }

    #[test]
    fn test_wasm_invoke_tool_definition() {
        let tool = wasm_invoke();
        assert_eq!(tool.name, "wasm_invoke");
        assert_eq!(tool.category, ToolCategory::Plugin);
        let required = tool.input_schema["required"]
            .as_array()
            .expect("required should be array");
        assert!(required.iter().any(|v| v == "plugin"));
        assert!(required.iter().any(|v| v == "function"));
    }

    #[test]
    fn test_tools_for_plugin_invoke_capability() {
        let caps = vec![Capability::PluginInvoke];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"wasm_invoke"));
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn test_all_tools_includes_wasm_invoke() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"wasm_invoke"));
    }

    // -----------------------------------------------------------------------
    // Self-configuration tool definition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_self_config_tools_registered() {
        let caps = vec![Capability::SelfConfig];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"heartbeat_add"), "missing heartbeat_add");
        assert!(names.contains(&"heartbeat_list"), "missing heartbeat_list");
        assert!(
            names.contains(&"heartbeat_remove"),
            "missing heartbeat_remove"
        );
        assert!(names.contains(&"creed_view"), "missing creed_view");
        assert!(names.contains(&"skill_list"), "missing skill_list");
        assert!(
            names.contains(&"skill_recommend"),
            "missing skill_recommend"
        );
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn test_self_config_tools_absent_without_capability() {
        let caps = vec![Capability::Memory];
        let tools = tools_for_capabilities(&caps);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"heartbeat_add"));
        assert!(!names.contains(&"creed_view"));
        assert!(!names.contains(&"skill_list"));
    }

    #[test]
    fn test_heartbeat_add_definition() {
        let t = heartbeat_add();
        assert_eq!(t.name, "heartbeat_add");
        assert_eq!(t.category, ToolCategory::Agent);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "task"));
        assert!(required.iter().any(|v| v == "cadence"));
    }

    #[test]
    fn test_heartbeat_remove_definition() {
        let t = heartbeat_remove();
        assert_eq!(t.name, "heartbeat_remove");
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "index"));
    }

    #[test]
    fn test_skill_recommend_definition() {
        let t = skill_recommend();
        assert_eq!(t.name, "skill_recommend");
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "pack_name"));
    }

    #[test]
    fn test_all_tools_includes_self_config() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"heartbeat_add"));
        assert!(names.contains(&"heartbeat_list"));
        assert!(names.contains(&"heartbeat_remove"));
        assert!(names.contains(&"creed_view"));
        assert!(names.contains(&"skill_list"));
        assert!(names.contains(&"skill_recommend"));
    }

    // --- Desktop automation tool definition tests ---

    #[test]
    fn test_sys_screenshot_definition() {
        let t = sys_screenshot();
        assert_eq!(t.name, "sys_screenshot");
        assert_eq!(t.category, ToolCategory::SystemAutomation);
        // window is optional (not in required)
        assert!(t.input_schema.get("required").is_none());
    }

    #[test]
    fn test_ui_screenshot_definition() {
        let t = ui_screenshot();
        assert_eq!(t.name, "ui_screenshot");
        assert_eq!(t.category, ToolCategory::UiAutomation);
    }

    #[test]
    fn test_app_ocr_definition() {
        let t = app_ocr();
        assert_eq!(t.name, "app_ocr");
        assert_eq!(t.category, ToolCategory::AppIntegration);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "app"));
    }

    #[test]
    fn test_ui_find_elements_definition() {
        let t = ui_find_elements();
        assert_eq!(t.name, "ui_find_elements");
        assert_eq!(t.category, ToolCategory::UiAutomation);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "app"));
    }

    #[test]
    fn test_ui_click_definition() {
        let t = ui_click();
        assert_eq!(t.name, "ui_click");
        assert_eq!(t.category, ToolCategory::UiAutomation);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "element_id"));
    }

    #[test]
    fn test_ui_type_text_definition() {
        let t = ui_type_text();
        assert_eq!(t.name, "ui_type_text");
        assert_eq!(t.category, ToolCategory::UiAutomation);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "element_id"));
        assert!(required.iter().any(|v| v == "text"));
    }

    #[test]
    fn test_ui_list_windows_definition() {
        let t = ui_list_windows();
        assert_eq!(t.name, "ui_list_windows");
        assert_eq!(t.category, ToolCategory::UiAutomation);
    }

    #[test]
    fn test_ui_read_attribute_definition() {
        let t = ui_read_attribute();
        assert_eq!(t.name, "ui_read_attribute");
        assert_eq!(t.category, ToolCategory::UiAutomation);
        let required = t.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "element_id"));
        assert!(required.iter().any(|v| v == "attribute"));
    }

    #[test]
    fn test_all_tools_includes_automation() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"sys_screenshot"));
        assert!(names.contains(&"ui_screenshot"));
        assert!(names.contains(&"app_ocr"));
        assert!(names.contains(&"ui_find_elements"));
        assert!(names.contains(&"ui_click"));
        assert!(names.contains(&"ui_type_text"));
        assert!(names.contains(&"ui_list_windows"));
        assert!(names.contains(&"ui_read_attribute"));
    }

    #[test]
    fn test_tools_for_system_automation_capability() {
        let tools = tools_for_capabilities(&[Capability::SystemAutomation]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"sys_screenshot"));
        assert!(!names.contains(&"ui_click")); // not included without UiAutomation
    }

    #[test]
    fn test_tools_for_ui_automation_capability() {
        let tools = tools_for_capabilities(&[Capability::UiAutomation("*".to_string())]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"ui_find_elements"));
        assert!(names.contains(&"ui_click"));
        assert!(names.contains(&"ui_type_text"));
        assert!(names.contains(&"ui_list_windows"));
        assert!(names.contains(&"ui_read_attribute"));
        assert!(names.contains(&"ui_screenshot"));
        assert!(!names.contains(&"sys_screenshot")); // not included without SystemAutomation
        assert!(!names.contains(&"app_ocr")); // not included without AppIntegration
    }

    #[test]
    fn test_tools_for_app_integration_capability() {
        let tools = tools_for_capabilities(&[Capability::AppIntegration("*".to_string())]);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"app_ocr"));
        assert!(!names.contains(&"ui_click")); // not included without UiAutomation
    }

    #[test]
    fn test_automation_tool_no_duplicates() {
        // Granting all automation capabilities should not produce duplicate tools.
        let tools = tools_for_capabilities(&[
            Capability::SystemAutomation,
            Capability::UiAutomation("*".to_string()),
            Capability::AppIntegration("*".to_string()),
        ]);
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tools found");
    }
}
