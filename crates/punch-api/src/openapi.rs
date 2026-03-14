//! OpenAPI 3.0.3 schema generator for the Punch Agent OS API.
//!
//! Builds the complete OpenAPI specification as a `serde_json::Value` without
//! any external OpenAPI crate — the JSON is assembled manually.

use serde_json::{json, Value};

/// Generate the complete OpenAPI 3.0.3 specification for the Punch API.
pub fn openapi_schema() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Punch Agent OS API",
            "description": "API for the Punch Agent Combat System — spawn fighters, manage gorillas, execute workflows, and coordinate troops.",
            "version": env!("CARGO_PKG_VERSION"),
            "contact": { "email": "team@humancto.com" }
        },
        "servers": [
            { "url": "http://localhost:6660", "description": "Local development" }
        ],
        "paths": paths(),
        "components": components()
    })
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn paths() -> Value {
    let mut paths = serde_json::Map::new();

    // Health & Status
    add_health_paths(&mut paths);
    // Fighters
    add_fighter_paths(&mut paths);
    // Gorillas
    add_gorilla_paths(&mut paths);
    // Workflows
    add_workflow_paths(&mut paths);
    // Chat / OpenAI compat
    add_chat_paths(&mut paths);
    // Troops
    add_troop_paths(&mut paths);
    // Channels
    add_channel_paths(&mut paths);
    // Triggers
    add_trigger_paths(&mut paths);
    // Dashboard
    add_dashboard_paths(&mut paths);
    // A2A
    add_a2a_paths(&mut paths);
    // Docs
    add_docs_paths(&mut paths);

    Value::Object(paths)
}

// ---------------------------------------------------------------------------
// Health & Status
// ---------------------------------------------------------------------------

fn add_health_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/health".to_string(), json!({
        "get": {
            "tags": ["Health"],
            "summary": "Health check",
            "description": "Returns a simple health check response indicating the Arena is running.",
            "operationId": "healthCheck",
            "security": [],
            "responses": {
                "200": {
                    "description": "Service is healthy",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "status": { "type": "string", "example": "ok" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }));

    paths.insert("/api/status".to_string(), json!({
        "get": {
            "tags": ["Health"],
            "summary": "System status",
            "description": "Returns detailed system status including fighter/gorilla counts and uptime.",
            "operationId": "systemStatus",
            "responses": {
                "200": {
                    "description": "System status",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "status": { "type": "string", "example": "ok" },
                                    "fighter_count": { "type": "integer" },
                                    "gorilla_count": { "type": "integer" },
                                    "uptime_secs": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Fighters
// ---------------------------------------------------------------------------

fn add_fighter_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/fighters".to_string(), json!({
        "post": {
            "tags": ["Fighters"],
            "summary": "Spawn a new fighter",
            "description": "Creates and registers a new fighter agent with the given manifest.",
            "operationId": "spawnFighter",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["manifest"],
                            "properties": {
                                "manifest": { "$ref": "#/components/schemas/FighterManifest" }
                            }
                        },
                        "example": {
                            "manifest": {
                                "name": "alpha",
                                "description": "General-purpose assistant",
                                "model": {
                                    "provider": "ollama",
                                    "model": "llama3",
                                    "max_tokens": 4096
                                },
                                "system_prompt": "You are a helpful assistant.",
                                "capabilities": [],
                                "weight_class": "middleweight"
                            }
                        }
                    }
                }
            },
            "responses": {
                "201": {
                    "description": "Fighter spawned",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "name": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "get": {
            "tags": ["Fighters"],
            "summary": "List all fighters",
            "description": "Returns a list of all registered fighters with their current status.",
            "operationId": "listFighters",
            "responses": {
                "200": {
                    "description": "List of fighters",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/FighterSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/fighters/{id}".to_string(), json!({
        "get": {
            "tags": ["Fighters"],
            "summary": "Get fighter details",
            "description": "Returns full details for a specific fighter, including its manifest and current status.",
            "operationId": "getFighter",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "Fighter details",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/FighterDetail" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "delete": {
            "tags": ["Fighters"],
            "summary": "Kill a fighter",
            "description": "Terminates and removes a fighter from the Ring.",
            "operationId": "killFighter",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "204": { "description": "Fighter killed" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/fighters/{id}/message".to_string(), json!({
        "post": {
            "tags": ["Fighters"],
            "summary": "Send message to fighter",
            "description": "Sends a text message to a fighter and returns its response, including token usage and tool call metrics.",
            "operationId": "sendMessage",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["message"],
                            "properties": {
                                "message": { "type": "string", "description": "The message to send to the fighter" }
                            }
                        },
                        "example": { "message": "Hello, who are you?" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Fighter response",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "response": { "type": "string" },
                                    "tokens_used": { "type": "integer" },
                                    "iterations": { "type": "integer" },
                                    "tool_calls_made": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" },
                "500": { "$ref": "#/components/responses/InternalError" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Gorillas
// ---------------------------------------------------------------------------

fn add_gorilla_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/gorillas".to_string(), json!({
        "get": {
            "tags": ["Gorillas"],
            "summary": "List gorillas",
            "description": "Returns a list of all registered gorillas with their schedule and status.",
            "operationId": "listGorillas",
            "responses": {
                "200": {
                    "description": "List of gorillas",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/GorillaSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/gorillas/{id}/unleash".to_string(), json!({
        "post": {
            "tags": ["Gorillas"],
            "summary": "Unleash a gorilla",
            "description": "Starts a gorilla, transitioning it from Caged to Rampaging status.",
            "operationId": "unleashGorilla",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": { "description": "Gorilla unleashed" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "409": {
                    "description": "Gorilla already active",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/Error" }
                        }
                    }
                },
                "429": { "$ref": "#/components/responses/RateLimited" },
                "500": { "$ref": "#/components/responses/InternalError" }
            }
        }
    }));

    paths.insert("/api/gorillas/{id}/cage".to_string(), json!({
        "post": {
            "tags": ["Gorillas"],
            "summary": "Cage a gorilla",
            "description": "Stops a gorilla, transitioning it back to Caged status.",
            "operationId": "cageGorilla",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": { "description": "Gorilla caged" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/gorillas/{id}/status".to_string(), json!({
        "get": {
            "tags": ["Gorillas"],
            "summary": "Get gorilla status",
            "description": "Returns the current status and metrics for a specific gorilla.",
            "operationId": "gorillaStatus",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "Gorilla status and metrics",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/GorillaStatusResponse" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Workflows
// ---------------------------------------------------------------------------

fn add_workflow_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/workflows".to_string(), json!({
        "post": {
            "tags": ["Workflows"],
            "summary": "Register a workflow",
            "description": "Creates a new multi-step workflow definition that can be executed later.",
            "operationId": "createWorkflow",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name", "steps"],
                            "properties": {
                                "name": { "type": "string" },
                                "steps": {
                                    "type": "array",
                                    "items": { "$ref": "#/components/schemas/WorkflowStepInput" }
                                }
                            }
                        },
                        "example": {
                            "name": "research-pipeline",
                            "steps": [
                                {
                                    "name": "research",
                                    "fighter_name": "researcher",
                                    "prompt_template": "Research the topic: {{input}}"
                                }
                            ]
                        }
                    }
                }
            },
            "responses": {
                "201": {
                    "description": "Workflow registered",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "name": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "get": {
            "tags": ["Workflows"],
            "summary": "List workflows",
            "description": "Returns all registered workflow definitions.",
            "operationId": "listWorkflows",
            "responses": {
                "200": {
                    "description": "List of workflows",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/WorkflowSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/workflows/{id}/execute".to_string(), json!({
        "post": {
            "tags": ["Workflows"],
            "summary": "Execute a workflow",
            "description": "Starts executing a registered workflow with the given input text.",
            "operationId": "executeWorkflow",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["input"],
                            "properties": {
                                "input": { "type": "string" }
                            }
                        },
                        "example": { "input": "Summarize the latest AI research trends" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Workflow execution started",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "run_id": { "type": "string", "format": "uuid" },
                                    "status": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" },
                "500": { "$ref": "#/components/responses/InternalError" }
            }
        }
    }));

    paths.insert("/api/workflows/{id}/runs".to_string(), json!({
        "get": {
            "tags": ["Workflows"],
            "summary": "List runs for a workflow",
            "description": "Returns all execution runs for a specific workflow.",
            "operationId": "listWorkflowRuns",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "List of workflow runs",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/WorkflowRun" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/workflows/{id}/runs/{run_id}".to_string(), json!({
        "get": {
            "tags": ["Workflows"],
            "summary": "Get workflow run status",
            "description": "Returns the status and results of a specific workflow run.",
            "operationId": "getWorkflowRun",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
                { "name": "run_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "Workflow run details",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/WorkflowRun" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Chat / OpenAI compat
// ---------------------------------------------------------------------------

fn add_chat_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/v1/chat/completions".to_string(), json!({
        "post": {
            "tags": ["Chat"],
            "summary": "Chat completion (OpenAI compatible)",
            "description": "OpenAI-compatible chat completion endpoint. The model field maps to a fighter name; if no matching fighter exists, a temporary one is spawned. Supports both streaming and non-streaming responses.",
            "operationId": "chatCompletion",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "$ref": "#/components/schemas/ChatCompletionRequest" },
                        "example": {
                            "model": "gpt-oss:20b",
                            "messages": [
                                { "role": "system", "content": "You are a helpful assistant." },
                                { "role": "user", "content": "Hello!" }
                            ],
                            "max_tokens": 4096,
                            "stream": false
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Chat completion response",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ChatCompletionResponse" }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" },
                "500": { "$ref": "#/components/responses/InternalError" }
            }
        }
    }));

    paths.insert("/v1/models".to_string(), json!({
        "get": {
            "tags": ["Chat"],
            "summary": "List available models",
            "description": "Returns available models in OpenAI format. Includes the configured default model, any Ollama models (if applicable), and active fighters.",
            "operationId": "listModels",
            "responses": {
                "200": {
                    "description": "Model list",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "object": { "type": "string", "example": "list" },
                                    "data": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "id": { "type": "string" },
                                                "object": { "type": "string", "example": "model" },
                                                "created": { "type": "integer" },
                                                "owned_by": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Troops
// ---------------------------------------------------------------------------

fn add_troop_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/troops".to_string(), json!({
        "post": {
            "tags": ["Troops"],
            "summary": "Form a troop",
            "description": "Creates a new troop — a coordinated group of fighters with a leader and strategy.",
            "operationId": "formTroop",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name", "leader", "members", "strategy"],
                            "properties": {
                                "name": { "type": "string" },
                                "leader": { "type": "string", "format": "uuid" },
                                "members": { "type": "array", "items": { "type": "string", "format": "uuid" } },
                                "strategy": { "type": "string", "enum": ["round_robin", "fan_out", "chain", "debate"] }
                            }
                        },
                        "example": {
                            "name": "research-team",
                            "leader": "00000000-0000-0000-0000-000000000001",
                            "members": ["00000000-0000-0000-0000-000000000002"],
                            "strategy": "fan_out"
                        }
                    }
                }
            },
            "responses": {
                "201": {
                    "description": "Troop formed",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "name": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "get": {
            "tags": ["Troops"],
            "summary": "List troops",
            "description": "Returns all registered troops with their strategy and status.",
            "operationId": "listTroops",
            "responses": {
                "200": {
                    "description": "List of troops",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/TroopSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/troops/{id}".to_string(), json!({
        "get": {
            "tags": ["Troops"],
            "summary": "Get troop details",
            "description": "Returns full details for a specific troop, including all members.",
            "operationId": "getTroop",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "Troop details",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/Troop" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "delete": {
            "tags": ["Troops"],
            "summary": "Disband troop",
            "description": "Disbands a troop, releasing all members.",
            "operationId": "disbandTroop",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "204": { "description": "Troop disbanded" },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/troops/{id}/tasks".to_string(), json!({
        "post": {
            "tags": ["Troops"],
            "summary": "Assign task to troop",
            "description": "Assigns a task to a troop. The task is distributed to members according to the troop's coordination strategy.",
            "operationId": "assignTroopTask",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["task"],
                            "properties": {
                                "task": { "type": "string" }
                            }
                        },
                        "example": { "task": "Research the latest developments in quantum computing" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Task assigned",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "assigned_to": {
                                        "type": "array",
                                        "items": { "type": "string", "format": "uuid" }
                                    }
                                }
                            }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/troops/{id}/members".to_string(), json!({
        "post": {
            "tags": ["Troops"],
            "summary": "Recruit a member",
            "description": "Adds a fighter to an existing troop.",
            "operationId": "recruitMember",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["fighter_id"],
                            "properties": {
                                "fighter_id": { "type": "string", "format": "uuid" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "204": { "description": "Member recruited" },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/troops/{troop_id}/members/{fighter_id}".to_string(), json!({
        "delete": {
            "tags": ["Troops"],
            "summary": "Dismiss a member",
            "description": "Removes a fighter from a troop.",
            "operationId": "dismissMember",
            "parameters": [
                { "name": "troop_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
                { "name": "fighter_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "204": { "description": "Member dismissed" },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

fn add_channel_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/channels/discord/webhook".to_string(), json!({
        "post": {
            "tags": ["Channels"],
            "summary": "Discord webhook",
            "description": "Receives incoming Discord messages and routes them to the appropriate fighter.",
            "operationId": "discordWebhook",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "description": "Discord webhook payload" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Webhook processed",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/WebhookResponse" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/channels/telegram/webhook".to_string(), json!({
        "post": {
            "tags": ["Channels"],
            "summary": "Telegram webhook",
            "description": "Receives incoming Telegram updates and routes them to the appropriate fighter.",
            "operationId": "telegramWebhook",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "description": "Telegram update payload" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Webhook processed",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/WebhookResponse" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/channels/slack/events".to_string(), json!({
        "post": {
            "tags": ["Channels"],
            "summary": "Slack events",
            "description": "Receives Slack Events API payloads (including URL verification challenges) and routes messages to fighters.",
            "operationId": "slackEvents",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "description": "Slack Events API payload" }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Event processed",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/WebhookResponse" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Triggers
// ---------------------------------------------------------------------------

fn add_trigger_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/triggers".to_string(), json!({
        "post": {
            "tags": ["Triggers"],
            "summary": "Create trigger",
            "description": "Registers a new trigger that fires actions based on conditions.",
            "operationId": "createTrigger",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name", "condition", "action"],
                            "properties": {
                                "name": { "type": "string" },
                                "condition": { "type": "object", "description": "Trigger condition definition" },
                                "action": { "type": "object", "description": "Trigger action definition" },
                                "max_fires": { "type": "integer", "default": 0, "description": "Maximum number of times the trigger can fire (0 = unlimited)" }
                            }
                        },
                        "example": {
                            "name": "daily-report",
                            "condition": { "type": "webhook" },
                            "action": { "type": "send_message", "fighter_name": "reporter", "message": "Generate daily report" },
                            "max_fires": 0
                        }
                    }
                }
            },
            "responses": {
                "201": {
                    "description": "Trigger registered",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "format": "uuid" },
                                    "name": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "get": {
            "tags": ["Triggers"],
            "summary": "List triggers",
            "description": "Returns all registered triggers with their current state.",
            "operationId": "listTriggers",
            "responses": {
                "200": {
                    "description": "List of triggers",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/TriggerListItem" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/triggers/{id}".to_string(), json!({
        "delete": {
            "tags": ["Triggers"],
            "summary": "Delete trigger",
            "description": "Removes a trigger by ID.",
            "operationId": "deleteTrigger",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "204": { "description": "Trigger deleted" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/triggers/webhook/{id}".to_string(), json!({
        "post": {
            "tags": ["Triggers"],
            "summary": "Receive webhook trigger",
            "description": "Webhook receiver endpoint that fires a trigger when called.",
            "operationId": "receiveWebhookTrigger",
            "parameters": [
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": {
                    "description": "Trigger fired",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "status": { "type": "string" },
                                    "action": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

fn add_dashboard_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert("/api/dashboard/status".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "Dashboard metrics",
            "description": "Returns arena-wide system overview including uptime, counts, active bouts, and system health.",
            "operationId": "dashboardStatus",
            "responses": {
                "200": {
                    "description": "Dashboard status",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/DashboardStatus" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/fighters".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "Dashboard fighter roster",
            "description": "Returns all fighters with their model and weight class for the dashboard display.",
            "operationId": "dashboardFighters",
            "responses": {
                "200": {
                    "description": "Fighter roster",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/DashboardFighterSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/gorillas".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "Dashboard gorilla enclosure",
            "description": "Returns all gorillas with their schedule and status for the dashboard display.",
            "operationId": "dashboardGorillas",
            "responses": {
                "200": {
                    "description": "Gorilla enclosure",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/DashboardGorillaSummary" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/audit".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "Audit log",
            "description": "Returns recent audit log entries from the event bus history.",
            "operationId": "dashboardAudit",
            "parameters": [
                { "name": "limit", "in": "query", "required": false, "schema": { "type": "integer", "default": 50 }, "description": "Maximum number of entries to return" },
                { "name": "since", "in": "query", "required": false, "schema": { "type": "integer", "default": 0 }, "description": "Only return entries after this sequence number" }
            ],
            "responses": {
                "200": {
                    "description": "Audit entries",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/AuditEntry" }
                            }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/metrics".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "System metrics",
            "description": "Returns aggregate token usage, tool call counts, and cost data from the metering engine.",
            "operationId": "dashboardMetrics",
            "responses": {
                "200": {
                    "description": "System metrics",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/DashboardMetrics" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/config".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "View configuration",
            "description": "Returns the current system configuration with API keys redacted.",
            "operationId": "dashboardConfig",
            "responses": {
                "200": {
                    "description": "Sanitized configuration",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/DashboardConfig" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        },
        "post": {
            "tags": ["Dashboard"],
            "summary": "Update configuration",
            "description": "Updates configuration settings (currently supports rate_limit_rpm). Some changes require a restart.",
            "operationId": "updateDashboardConfig",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "rate_limit_rpm": { "type": "integer", "minimum": 1, "maximum": 100000 }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Configuration updated",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "message": { "type": "string" },
                                    "applied": { "type": "boolean" }
                                }
                            }
                        }
                    }
                },
                "400": { "$ref": "#/components/responses/BadRequest" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "429": { "$ref": "#/components/responses/RateLimited" }
            }
        }
    }));

    paths.insert("/api/dashboard/events".to_string(), json!({
        "get": {
            "tags": ["Dashboard"],
            "summary": "Real-time event stream",
            "description": "WebSocket upgrade endpoint for streaming real-time events from the Ring's event bus.",
            "operationId": "dashboardEvents",
            "responses": {
                "101": { "description": "WebSocket upgrade" },
                "401": { "$ref": "#/components/responses/Unauthorized" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// A2A (Agent-to-Agent)
// ---------------------------------------------------------------------------

fn add_a2a_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert(
        "/.well-known/agent.json".to_string(),
        json!({
            "get": {
                "tags": ["A2A"],
                "summary": "Agent card",
                "description": "Returns this agent's A2A discovery card.",
                "operationId": "getAgentCard",
                "security": [],
                "responses": {
                    "200": {
                        "description": "Agent card",
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AgentCard" }
                            }
                        }
                    }
                }
            }
        }),
    );

    paths.insert(
        "/a2a/tasks/send".to_string(),
        json!({
            "post": {
                "tags": ["A2A"],
                "summary": "Send task to agent",
                "description": "Sends a task to this agent for execution via the A2A protocol.",
                "operationId": "sendA2ATask",
                "requestBody": {
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["message"],
                                "properties": {
                                    "message": { "type": "string" },
                                    "from_agent": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "responses": {
                    "200": {
                        "description": "Task accepted",
                        "content": {
                            "application/json": {
                                "schema": { "type": "object" }
                            }
                        }
                    },
                    "401": { "$ref": "#/components/responses/Unauthorized" },
                    "429": { "$ref": "#/components/responses/RateLimited" }
                }
            }
        }),
    );

    paths.insert("/a2a/tasks/{task_id}".to_string(), json!({
        "get": {
            "tags": ["A2A"],
            "summary": "Get task status",
            "description": "Returns the status and result of an A2A task.",
            "operationId": "getA2ATask",
            "parameters": [
                { "name": "task_id", "in": "path", "required": true, "schema": { "type": "string" } }
            ],
            "responses": {
                "200": {
                    "description": "Task status",
                    "content": {
                        "application/json": {
                            "schema": { "type": "object" }
                        }
                    }
                },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" }
            }
        }
    }));

    paths.insert("/a2a/tasks/{task_id}/cancel".to_string(), json!({
        "post": {
            "tags": ["A2A"],
            "summary": "Cancel task",
            "description": "Cancels a pending or running A2A task.",
            "operationId": "cancelA2ATask",
            "parameters": [
                { "name": "task_id", "in": "path", "required": true, "schema": { "type": "string" } }
            ],
            "responses": {
                "200": { "description": "Task cancelled" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "404": { "$ref": "#/components/responses/NotFound" }
            }
        }
    }));

    paths.insert(
        "/a2a/register".to_string(),
        json!({
            "post": {
                "tags": ["A2A"],
                "summary": "Register remote agent",
                "description": "Registers a remote agent's card for discovery.",
                "operationId": "registerAgent",
                "requestBody": {
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/AgentCard" }
                        }
                    }
                },
                "responses": {
                    "200": { "description": "Agent registered" },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        }),
    );

    paths.insert(
        "/a2a/agents".to_string(),
        json!({
            "get": {
                "tags": ["A2A"],
                "summary": "List registered agents",
                "description": "Returns all registered remote agents.",
                "operationId": "listAgents",
                "responses": {
                    "200": {
                        "description": "Agent list",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "array",
                                    "items": { "$ref": "#/components/schemas/AgentCard" }
                                }
                            }
                        }
                    },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        }),
    );

    paths.insert("/a2a/agents/{agent_id}".to_string(), json!({
        "delete": {
            "tags": ["A2A"],
            "summary": "Remove remote agent",
            "description": "Removes a registered remote agent.",
            "operationId": "removeAgent",
            "parameters": [
                { "name": "agent_id", "in": "path", "required": true, "schema": { "type": "string" } }
            ],
            "responses": {
                "204": { "description": "Agent removed" },
                "401": { "$ref": "#/components/responses/Unauthorized" }
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------

fn add_docs_paths(paths: &mut serde_json::Map<String, Value>) {
    paths.insert(
        "/api/openapi.json".to_string(),
        json!({
            "get": {
                "tags": ["Documentation"],
                "summary": "OpenAPI schema",
                "description": "Returns the OpenAPI 3.0.3 JSON schema for the Punch API.",
                "operationId": "getOpenApiSchema",
                "security": [],
                "responses": {
                    "200": {
                        "description": "OpenAPI schema",
                        "content": {
                            "application/json": {
                                "schema": { "type": "object" }
                            }
                        }
                    }
                }
            }
        }),
    );

    paths.insert(
        "/api/docs".to_string(),
        json!({
            "get": {
                "tags": ["Documentation"],
                "summary": "API documentation",
                "description": "Returns a Swagger UI page for interactive API exploration.",
                "operationId": "getApiDocs",
                "security": [],
                "responses": {
                    "200": {
                        "description": "Swagger UI HTML page",
                        "content": {
                            "text/html": {
                                "schema": { "type": "string" }
                            }
                        }
                    }
                }
            }
        }),
    );
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

fn components() -> Value {
    json!({
        "schemas": component_schemas(),
        "securitySchemes": {
            "bearerAuth": {
                "type": "http",
                "scheme": "bearer",
                "description": "API key passed as a Bearer token."
            }
        },
        "responses": common_responses()
    })
}

fn common_responses() -> Value {
    json!({
        "BadRequest": {
            "description": "Invalid request body or parameters",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/Error" }
                }
            }
        },
        "Unauthorized": {
            "description": "Missing or invalid API key",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/Error" }
                }
            }
        },
        "NotFound": {
            "description": "Resource not found",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/Error" }
                }
            }
        },
        "RateLimited": {
            "description": "Rate limit exceeded",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/Error" }
                }
            }
        },
        "InternalError": {
            "description": "Internal server error",
            "content": {
                "application/json": {
                    "schema": { "$ref": "#/components/schemas/Error" }
                }
            }
        }
    })
}

fn component_schemas() -> Value {
    json!({
        "Error": {
            "type": "object",
            "properties": {
                "error": { "type": "string", "description": "Error message" }
            },
            "required": ["error"]
        },
        "FighterManifest": {
            "type": "object",
            "required": ["name", "description", "model", "system_prompt", "weight_class"],
            "properties": {
                "name": { "type": "string", "description": "Unique fighter name" },
                "description": { "type": "string" },
                "model": { "$ref": "#/components/schemas/ModelConfig" },
                "system_prompt": { "type": "string", "description": "System prompt for the fighter" },
                "capabilities": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of capability names"
                },
                "weight_class": {
                    "type": "string",
                    "enum": ["flyweight", "bantamweight", "featherweight", "lightweight", "welterweight", "middleweight", "heavyweight"],
                    "description": "Fighter capability tier"
                }
            }
        },
        "ModelConfig": {
            "type": "object",
            "required": ["provider", "model"],
            "properties": {
                "provider": { "type": "string", "enum": ["openai", "anthropic", "ollama", "google", "custom"] },
                "model": { "type": "string", "description": "Model name or identifier" },
                "api_key_env": { "type": "string", "description": "Environment variable name for the API key" },
                "base_url": { "type": "string", "description": "Custom API base URL" },
                "max_tokens": { "type": "integer", "description": "Maximum tokens to generate" },
                "temperature": { "type": "number", "format": "float", "description": "Sampling temperature" }
            }
        },
        "FighterSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "weight_class": { "type": "string" },
                "status": { "type": "string", "enum": ["idle", "fighting", "resting", "knocked_out"] }
            }
        },
        "FighterDetail": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "manifest": { "$ref": "#/components/schemas/FighterManifest" },
                "status": { "type": "string", "enum": ["idle", "fighting", "resting", "knocked_out"] }
            }
        },
        "GorillaSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "schedule": { "type": "string", "description": "Cron schedule expression" },
                "status": { "type": "string", "enum": ["caged", "rampaging"] }
            }
        },
        "GorillaStatusResponse": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "status": { "type": "string", "enum": ["caged", "rampaging"] },
                "metrics": {
                    "type": "object",
                    "properties": {
                        "tasks_completed": { "type": "integer" },
                        "uptime_secs": { "type": "integer" },
                        "last_rampage": { "type": "string", "format": "date-time", "nullable": true }
                    }
                }
            }
        },
        "WorkflowStepInput": {
            "type": "object",
            "required": ["name", "fighter_name", "prompt_template"],
            "properties": {
                "name": { "type": "string" },
                "fighter_name": { "type": "string", "description": "Name of the fighter to execute this step" },
                "prompt_template": { "type": "string", "description": "Prompt template with {{input}} placeholder" },
                "timeout_secs": { "type": "integer", "description": "Step timeout in seconds" },
                "on_error": { "type": "string", "enum": ["fail_workflow", "skip_step", "retry_once"], "default": "fail_workflow" }
            }
        },
        "WorkflowSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "step_count": { "type": "integer" }
            }
        },
        "WorkflowRun": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "workflow_id": { "type": "string", "format": "uuid" },
                "status": { "type": "string", "enum": ["pending", "running", "completed", "failed"] },
                "started_at": { "type": "string", "format": "date-time" },
                "completed_at": { "type": "string", "format": "date-time", "nullable": true },
                "step_results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step_name": { "type": "string" },
                            "output": { "type": "string" },
                            "status": { "type": "string" }
                        }
                    }
                }
            }
        },
        "Troop": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "leader": { "type": "string", "format": "uuid" },
                "members": {
                    "type": "array",
                    "items": { "type": "string", "format": "uuid" }
                },
                "strategy": { "type": "string", "enum": ["round_robin", "fan_out", "chain", "debate"] },
                "status": { "type": "string", "enum": ["active", "disbanded"] }
            }
        },
        "TroopSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "leader": { "type": "string", "format": "uuid" },
                "member_count": { "type": "integer" },
                "strategy": { "type": "string" },
                "status": { "type": "string" }
            }
        },
        "ChatCompletionRequest": {
            "type": "object",
            "required": ["model", "messages"],
            "properties": {
                "model": { "type": "string", "description": "Model name — maps to a fighter name or the configured model" },
                "messages": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/Message" }
                },
                "stream": { "type": "boolean", "default": false },
                "max_tokens": { "type": "integer" },
                "temperature": { "type": "number", "format": "float" },
                "tools": {
                    "type": "array",
                    "items": { "type": "object" },
                    "description": "Tool definitions (accepted but not used for routing)"
                },
                "tool_choice": { "description": "Tool choice strategy" }
            }
        },
        "ChatCompletionResponse": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "object": { "type": "string", "example": "chat.completion" },
                "created": { "type": "integer" },
                "model": { "type": "string" },
                "choices": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "index": { "type": "integer" },
                            "message": {
                                "type": "object",
                                "properties": {
                                    "role": { "type": "string" },
                                    "content": { "type": "string", "nullable": true },
                                    "tool_calls": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/ToolCall" },
                                        "nullable": true
                                    }
                                }
                            },
                            "finish_reason": { "type": "string" }
                        }
                    }
                },
                "usage": {
                    "type": "object",
                    "properties": {
                        "prompt_tokens": { "type": "integer" },
                        "completion_tokens": { "type": "integer" },
                        "total_tokens": { "type": "integer" }
                    }
                }
            }
        },
        "Message": {
            "type": "object",
            "required": ["role"],
            "properties": {
                "role": { "type": "string", "enum": ["system", "user", "assistant", "tool"] },
                "content": { "type": "string", "nullable": true },
                "tool_calls": {
                    "type": "array",
                    "items": { "$ref": "#/components/schemas/ToolCall" },
                    "nullable": true
                },
                "tool_call_id": { "type": "string", "nullable": true }
            }
        },
        "ToolCall": {
            "type": "object",
            "properties": {
                "index": { "type": "integer", "nullable": true },
                "id": { "type": "string", "nullable": true },
                "type": { "type": "string", "example": "function" },
                "function": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "arguments": { "type": "string" }
                    }
                }
            }
        },
        "WebhookResponse": {
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "response": { "type": "string", "nullable": true },
                "error": { "type": "string", "nullable": true }
            }
        },
        "TriggerListItem": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "name": { "type": "string" },
                "condition_type": { "type": "string" },
                "enabled": { "type": "boolean" },
                "fire_count": { "type": "integer" },
                "created_at": { "type": "string", "format": "date-time" }
            }
        },
        "DashboardStatus": {
            "type": "object",
            "properties": {
                "uptime_secs": { "type": "integer" },
                "fighter_count": { "type": "integer" },
                "gorilla_count": { "type": "integer" },
                "active_bouts": { "type": "integer" },
                "total_messages": { "type": "integer" },
                "memory_entries": { "type": "integer" },
                "system_health": { "type": "string" }
            }
        },
        "DashboardFighterSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "weight_class": { "type": "string" },
                "status": { "type": "string" },
                "model": { "type": "string" }
            }
        },
        "DashboardGorillaSummary": {
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "schedule": { "type": "string" },
                "status": { "type": "string" },
                "last_rampage": { "type": "string", "nullable": true }
            }
        },
        "AuditEntry": {
            "type": "object",
            "properties": {
                "sequence": { "type": "integer" },
                "timestamp": { "type": "string", "format": "date-time" },
                "kind": { "type": "string" },
                "summary": { "type": "string" }
            }
        },
        "DashboardMetrics": {
            "type": "object",
            "properties": {
                "total_tokens_used": { "type": "integer" },
                "total_tool_calls": { "type": "integer" },
                "total_cost_usd": { "type": "number" },
                "fighter_count": { "type": "integer" },
                "gorilla_count": { "type": "integer" }
            }
        },
        "DashboardConfig": {
            "type": "object",
            "properties": {
                "api_listen": { "type": "string" },
                "api_key_status": { "type": "string" },
                "rate_limit_rpm": { "type": "integer" },
                "default_model": {
                    "type": "object",
                    "properties": {
                        "provider": { "type": "string" },
                        "model": { "type": "string" },
                        "api_key_env": { "type": "string", "nullable": true },
                        "base_url": { "type": "string", "nullable": true },
                        "max_tokens": { "type": "integer", "nullable": true },
                        "temperature": { "type": "number", "nullable": true }
                    }
                },
                "memory_db_path": { "type": "string" },
                "knowledge_graph_enabled": { "type": "boolean" },
                "channel_count": { "type": "integer" },
                "mcp_server_count": { "type": "integer" }
            }
        },
        "AgentCard": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "url": { "type": "string" },
                "capabilities": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "version": { "type": "string" }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Swagger UI HTML
// ---------------------------------------------------------------------------

/// Returns a self-contained HTML page that loads Swagger UI from CDN.
pub fn swagger_ui_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head>
  <title>Punch API Documentation</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist/swagger-ui-bundle.js"></script>
  <script>
    SwaggerUI({ url: '/api/openapi.json', dom_id: '#swagger-ui' });
  </script>
</body>
</html>"#
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_schema_is_valid_json() {
        let schema = openapi_schema();
        // Verify it can be serialized and deserialized.
        let json_str = serde_json::to_string_pretty(&schema).expect("serialization should work");
        let _: Value = serde_json::from_str(&json_str).expect("should be valid JSON");
    }

    #[test]
    fn test_openapi_required_fields() {
        let schema = openapi_schema();
        assert!(schema.get("openapi").is_some(), "missing openapi field");
        assert!(schema.get("info").is_some(), "missing info field");
        assert!(schema.get("paths").is_some(), "missing paths field");
        assert_eq!(schema["openapi"], "3.0.3");
    }

    #[test]
    fn test_openapi_info_fields() {
        let schema = openapi_schema();
        let info = &schema["info"];
        assert_eq!(info["title"], "Punch Agent OS API");
        assert!(info["description"].is_string());
        assert!(info["version"].is_string());
        assert_eq!(info["contact"]["email"], "team@humancto.com");
    }

    #[test]
    fn test_schema_version_matches_package() {
        let schema = openapi_schema();
        assert_eq!(schema["info"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_fighters_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/fighters").is_some(),
            "missing /api/fighters"
        );
        assert!(
            paths.get("/api/fighters/{id}").is_some(),
            "missing /api/fighters/{{id}}"
        );
        assert!(
            paths.get("/api/fighters/{id}/message").is_some(),
            "missing /api/fighters/{{id}}/message"
        );
    }

    #[test]
    fn test_gorillas_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/gorillas").is_some(),
            "missing /api/gorillas"
        );
        assert!(
            paths.get("/api/gorillas/{id}/unleash").is_some(),
            "missing /api/gorillas/{{id}}/unleash"
        );
        assert!(
            paths.get("/api/gorillas/{id}/cage").is_some(),
            "missing /api/gorillas/{{id}}/cage"
        );
        assert!(
            paths.get("/api/gorillas/{id}/status").is_some(),
            "missing /api/gorillas/{{id}}/status"
        );
    }

    #[test]
    fn test_workflows_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/workflows").is_some(),
            "missing /api/workflows"
        );
        assert!(
            paths.get("/api/workflows/{id}/execute").is_some(),
            "missing /api/workflows/{{id}}/execute"
        );
        assert!(
            paths.get("/api/workflows/{id}/runs").is_some(),
            "missing /api/workflows/{{id}}/runs"
        );
        assert!(
            paths.get("/api/workflows/{id}/runs/{run_id}").is_some(),
            "missing /api/workflows/{{id}}/runs/{{run_id}}"
        );
    }

    #[test]
    fn test_chat_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/v1/chat/completions").is_some(),
            "missing /v1/chat/completions"
        );
        assert!(paths.get("/v1/models").is_some(), "missing /v1/models");
    }

    #[test]
    fn test_troops_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(paths.get("/api/troops").is_some(), "missing /api/troops");
        assert!(
            paths.get("/api/troops/{id}").is_some(),
            "missing /api/troops/{{id}}"
        );
        assert!(
            paths.get("/api/troops/{id}/tasks").is_some(),
            "missing /api/troops/{{id}}/tasks"
        );
        assert!(
            paths.get("/api/troops/{id}/members").is_some(),
            "missing /api/troops/{{id}}/members"
        );
        assert!(
            paths
                .get("/api/troops/{troop_id}/members/{fighter_id}")
                .is_some(),
            "missing /api/troops/{{troop_id}}/members/{{fighter_id}}"
        );
    }

    #[test]
    fn test_channels_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/channels/discord/webhook").is_some(),
            "missing discord webhook"
        );
        assert!(
            paths.get("/api/channels/telegram/webhook").is_some(),
            "missing telegram webhook"
        );
        assert!(
            paths.get("/api/channels/slack/events").is_some(),
            "missing slack events"
        );
    }

    #[test]
    fn test_triggers_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/triggers").is_some(),
            "missing /api/triggers"
        );
        assert!(
            paths.get("/api/triggers/{id}").is_some(),
            "missing /api/triggers/{{id}}"
        );
        assert!(
            paths.get("/api/triggers/webhook/{id}").is_some(),
            "missing /api/triggers/webhook/{{id}}"
        );
    }

    #[test]
    fn test_dashboard_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/dashboard/status").is_some(),
            "missing /api/dashboard/status"
        );
        assert!(
            paths.get("/api/dashboard/fighters").is_some(),
            "missing /api/dashboard/fighters"
        );
        assert!(
            paths.get("/api/dashboard/gorillas").is_some(),
            "missing /api/dashboard/gorillas"
        );
        assert!(
            paths.get("/api/dashboard/audit").is_some(),
            "missing /api/dashboard/audit"
        );
        assert!(
            paths.get("/api/dashboard/metrics").is_some(),
            "missing /api/dashboard/metrics"
        );
        assert!(
            paths.get("/api/dashboard/config").is_some(),
            "missing /api/dashboard/config"
        );
        assert!(
            paths.get("/api/dashboard/events").is_some(),
            "missing /api/dashboard/events"
        );
    }

    #[test]
    fn test_a2a_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/.well-known/agent.json").is_some(),
            "missing agent card"
        );
        assert!(
            paths.get("/a2a/tasks/send").is_some(),
            "missing /a2a/tasks/send"
        );
        assert!(
            paths.get("/a2a/tasks/{task_id}").is_some(),
            "missing /a2a/tasks/{{task_id}}"
        );
        assert!(
            paths.get("/a2a/register").is_some(),
            "missing /a2a/register"
        );
        assert!(paths.get("/a2a/agents").is_some(), "missing /a2a/agents");
    }

    #[test]
    fn test_health_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(paths.get("/health").is_some(), "missing /health");
        assert!(paths.get("/api/status").is_some(), "missing /api/status");
    }

    #[test]
    fn test_docs_paths_exist() {
        let schema = openapi_schema();
        let paths = &schema["paths"];
        assert!(
            paths.get("/api/openapi.json").is_some(),
            "missing /api/openapi.json"
        );
        assert!(paths.get("/api/docs").is_some(), "missing /api/docs");
    }

    #[test]
    fn test_component_schemas_exist() {
        let schema = openapi_schema();
        let schemas = &schema["components"]["schemas"];
        let expected = [
            "Error",
            "FighterManifest",
            "ModelConfig",
            "FighterSummary",
            "FighterDetail",
            "GorillaSummary",
            "GorillaStatusResponse",
            "WorkflowStepInput",
            "WorkflowSummary",
            "WorkflowRun",
            "Troop",
            "TroopSummary",
            "ChatCompletionRequest",
            "ChatCompletionResponse",
            "Message",
            "ToolCall",
            "WebhookResponse",
            "TriggerListItem",
            "DashboardStatus",
            "DashboardMetrics",
            "DashboardConfig",
            "AgentCard",
            "AuditEntry",
        ];
        for name in expected {
            assert!(
                schemas.get(name).is_some(),
                "missing component schema: {}",
                name
            );
        }
    }

    #[test]
    fn test_security_schemes_exist() {
        let schema = openapi_schema();
        let sec = &schema["components"]["securitySchemes"];
        assert!(sec.get("bearerAuth").is_some(), "missing bearerAuth scheme");
        assert_eq!(sec["bearerAuth"]["type"], "http");
        assert_eq!(sec["bearerAuth"]["scheme"], "bearer");
    }

    #[test]
    fn test_response_codes_documented() {
        let schema = openapi_schema();
        let paths = &schema["paths"];

        // POST /api/fighters should have 201 response
        assert!(
            paths["/api/fighters"]["post"]["responses"]
                .get("201")
                .is_some(),
            "missing 201 on POST /api/fighters"
        );

        // GET /api/fighters/{id} should have 404 response
        assert!(
            paths["/api/fighters/{id}"]["get"]["responses"]
                .get("404")
                .is_some(),
            "missing 404 on GET /api/fighters/{{id}}"
        );

        // DELETE /api/fighters/{id} should have 204 response
        assert!(
            paths["/api/fighters/{id}"]["delete"]["responses"]
                .get("204")
                .is_some(),
            "missing 204 on DELETE /api/fighters/{{id}}"
        );

        // POST /api/fighters/{id}/message should have 500 response
        assert!(
            paths["/api/fighters/{id}/message"]["post"]["responses"]
                .get("500")
                .is_some(),
            "missing 500 on POST /api/fighters/{{id}}/message"
        );
    }

    #[test]
    fn test_common_responses_defined() {
        let schema = openapi_schema();
        let responses = &schema["components"]["responses"];
        let expected = [
            "BadRequest",
            "Unauthorized",
            "NotFound",
            "RateLimited",
            "InternalError",
        ];
        for name in expected {
            assert!(
                responses.get(name).is_some(),
                "missing common response: {}",
                name
            );
        }
    }

    #[test]
    fn test_swagger_ui_html_contains_required_elements() {
        let html = swagger_ui_html();
        assert!(html.contains("<!DOCTYPE html>"), "missing DOCTYPE");
        assert!(
            html.contains("swagger-ui-dist/swagger-ui.css"),
            "missing CSS link"
        );
        assert!(
            html.contains("swagger-ui-dist/swagger-ui-bundle.js"),
            "missing JS script"
        );
        assert!(html.contains("swagger-ui"), "missing swagger-ui div");
        assert!(html.contains("/api/openapi.json"), "missing schema URL");
        assert!(html.contains("Punch API Documentation"), "missing title");
    }

    #[test]
    fn test_all_endpoint_groups_covered() {
        let schema = openapi_schema();
        let paths = &schema["paths"];

        // Collect all tags used
        let mut tags = std::collections::HashSet::new();
        if let Some(paths_obj) = paths.as_object() {
            for (_path, methods) in paths_obj {
                if let Some(methods_obj) = methods.as_object() {
                    for (_method, spec) in methods_obj {
                        if let Some(tag_arr) = spec.get("tags").and_then(|t| t.as_array()) {
                            for tag in tag_arr {
                                if let Some(t) = tag.as_str() {
                                    tags.insert(t.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        let required_groups = [
            "Fighters",
            "Gorillas",
            "Workflows",
            "Chat",
            "Troops",
            "Channels",
            "Triggers",
            "Health",
            "Dashboard",
            "A2A",
            "Documentation",
        ];
        for group in required_groups {
            assert!(tags.contains(group), "missing endpoint group: {}", group);
        }
    }

    #[test]
    fn test_servers_field_present() {
        let schema = openapi_schema();
        let servers = schema["servers"]
            .as_array()
            .expect("servers should be array");
        assert!(!servers.is_empty());
        assert_eq!(servers[0]["url"], "http://localhost:6660");
    }
}
