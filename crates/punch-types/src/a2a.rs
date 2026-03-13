//! # Agent-to-Agent (A2A) Protocol
//!
//! Inter-system communication protocol for agent discovery and task delegation.
//! Think of it like a fight card — each agent publishes its card so others know
//! its weight class, special moves, and how to reach it in the ring.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Authentication method for reaching a remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum A2AAuth {
    /// Bearer token authentication.
    Bearer(String),
    /// API key authentication.
    ApiKey(String),
    /// No authentication required.
    None,
}

// ---------------------------------------------------------------------------
// Agent Card (fight card)
// ---------------------------------------------------------------------------

/// An agent's public identity card — its fight card.
///
/// Published so other agents can discover capabilities, supported I/O modes,
/// and how to send tasks to this fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    /// Human-readable name of the agent.
    pub name: String,
    /// Description of what this agent does.
    pub description: String,
    /// The URL where this agent can be reached.
    pub url: String,
    /// Semantic version of the agent.
    pub version: String,
    /// List of capability identifiers (e.g. "code_review", "web_search").
    pub capabilities: Vec<String>,
    /// Supported input modes (e.g. "text", "json", "image").
    pub input_modes: Vec<String>,
    /// Supported output modes (e.g. "text", "json", "markdown").
    pub output_modes: Vec<String>,
    /// Optional authentication details for reaching this agent.
    pub authentication: Option<A2AAuth>,
}

// ---------------------------------------------------------------------------
// Task types
// ---------------------------------------------------------------------------

/// Status of an A2A task as it moves through the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum A2ATaskStatus {
    /// Task is queued but not yet started.
    Pending,
    /// Task is actively being processed.
    Running,
    /// Task finished successfully.
    Completed,
    /// Task failed with an error message.
    Failed(String),
    /// Task was cancelled before completion.
    Cancelled,
}

/// A task sent from one agent to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2ATask {
    /// Unique identifier for this task.
    pub id: String,
    /// Current status.
    pub status: A2ATaskStatus,
    /// Input payload (JSON).
    pub input: serde_json::Value,
    /// Output payload, populated on completion.
    pub output: Option<serde_json::Value>,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
    /// When the task was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Structured input payload for an A2A task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2ATaskInput {
    /// The prompt or instruction to execute.
    pub prompt: String,
    /// Optional additional context as key-value pairs.
    #[serde(default)]
    pub context: serde_json::Map<String, serde_json::Value>,
    /// Input mode (e.g. "text", "json").
    #[serde(default = "default_mode")]
    pub mode: String,
}

/// Structured output payload from an A2A task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2ATaskOutput {
    /// The result content.
    pub content: String,
    /// Optional structured data.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Output mode (e.g. "text", "json").
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "text".to_string()
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// A message exchanged during an A2A task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    /// The task this message belongs to.
    pub task_id: String,
    /// Role of the sender (e.g. "user", "agent").
    pub role: String,
    /// The message content.
    pub content: String,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Client trait
// ---------------------------------------------------------------------------

/// Client interface for A2A protocol operations.
///
/// Implementations handle the transport layer (HTTP, gRPC, etc.) to communicate
/// with remote agents.
#[async_trait]
pub trait A2AClient: Send + Sync {
    /// Discover a remote agent by fetching its card from a URL.
    async fn discover(&self, url: &str) -> PunchResult<AgentCard>;

    /// Send a task to a remote agent for execution.
    async fn send_task(&self, agent: &AgentCard, task: A2ATask) -> PunchResult<A2ATask>;

    /// Poll the status of a previously submitted task.
    async fn get_task_status(&self, agent: &AgentCard, task_id: &str)
    -> PunchResult<A2ATaskStatus>;

    /// Cancel a running task on a remote agent.
    async fn cancel_task(&self, agent: &AgentCard, task_id: &str) -> PunchResult<()>;
}

// ---------------------------------------------------------------------------
// HTTP Client Implementation
// ---------------------------------------------------------------------------

/// Default timeout for HTTP A2A requests (30 seconds).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP-based implementation of the A2A client protocol.
///
/// Uses reqwest to make real HTTP calls to remote agents' A2A endpoints.
pub struct HttpA2AClient {
    client: reqwest::Client,
}

impl HttpA2AClient {
    /// Create a new HTTP A2A client with the default 30-second timeout.
    pub fn new() -> PunchResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| PunchError::Internal(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { client })
    }

    /// Create a new HTTP A2A client with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> PunchResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| PunchError::Internal(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { client })
    }

    /// Build the full URL for an A2A endpoint on a remote agent.
    pub fn build_url(base_url: &str, path: &str) -> String {
        let base = base_url.trim_end_matches('/');
        format!("{base}{path}")
    }
}

impl Default for HttpA2AClient {
    fn default() -> Self {
        Self::new().expect("failed to create default HttpA2AClient")
    }
}

#[async_trait]
impl A2AClient for HttpA2AClient {
    /// Fetch a remote agent's card from its well-known URL.
    async fn discover(&self, url: &str) -> PunchResult<AgentCard> {
        let card_url = Self::build_url(url, "/.well-known/agent.json");
        let resp = self
            .client
            .get(&card_url)
            .send()
            .await
            .map_err(|e| PunchError::Internal(format!("A2A discover failed for {url}: {e}")))?;

        if !resp.status().is_success() {
            return Err(PunchError::Internal(format!(
                "A2A discover returned {} for {card_url}",
                resp.status()
            )));
        }

        resp.json::<AgentCard>()
            .await
            .map_err(|e| PunchError::Internal(format!("A2A discover parse error: {e}")))
    }

    /// Send a task to a remote agent via HTTP POST.
    async fn send_task(&self, agent: &AgentCard, task: A2ATask) -> PunchResult<A2ATask> {
        let url = Self::build_url(&agent.url, "/a2a/tasks/send");
        let resp = self
            .client
            .post(&url)
            .json(&task)
            .send()
            .await
            .map_err(|e| {
                PunchError::Internal(format!("A2A send_task failed for {}: {e}", agent.name))
            })?;

        if !resp.status().is_success() {
            return Err(PunchError::Internal(format!(
                "A2A send_task returned {} for {}",
                resp.status(),
                agent.name
            )));
        }

        resp.json::<A2ATask>()
            .await
            .map_err(|e| PunchError::Internal(format!("A2A send_task parse error: {e}")))
    }

    /// Get the status of a task on a remote agent via HTTP GET.
    async fn get_task_status(
        &self,
        agent: &AgentCard,
        task_id: &str,
    ) -> PunchResult<A2ATaskStatus> {
        let url = Self::build_url(&agent.url, &format!("/a2a/tasks/{task_id}"));
        let resp = self.client.get(&url).send().await.map_err(|e| {
            PunchError::Internal(format!(
                "A2A get_task_status failed for {}: {e}",
                agent.name
            ))
        })?;

        if !resp.status().is_success() {
            return Err(PunchError::Internal(format!(
                "A2A get_task_status returned {} for {}",
                resp.status(),
                agent.name
            )));
        }

        let task = resp
            .json::<A2ATask>()
            .await
            .map_err(|e| PunchError::Internal(format!("A2A get_task_status parse error: {e}")))?;

        Ok(task.status)
    }

    /// Cancel a task on a remote agent via HTTP POST.
    async fn cancel_task(&self, agent: &AgentCard, task_id: &str) -> PunchResult<()> {
        let url = Self::build_url(&agent.url, &format!("/a2a/tasks/{task_id}/cancel"));
        let resp = self.client.post(&url).send().await.map_err(|e| {
            PunchError::Internal(format!("A2A cancel_task failed for {}: {e}", agent.name))
        })?;

        if !resp.status().is_success() {
            return Err(PunchError::Internal(format!(
                "A2A cancel_task returned {} for {}",
                resp.status(),
                agent.name
            )));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Thread-safe registry of known agent cards.
///
/// Acts as the fight roster — keeps track of all agents that have checked in
/// so we can discover and delegate to them.
pub struct A2ARegistry {
    agents: DashMap<String, AgentCard>,
}

impl A2ARegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            agents: DashMap::new(),
        }
    }

    /// Register an agent card (or overwrite an existing one with the same name).
    pub fn register(&self, card: AgentCard) {
        self.agents.insert(card.name.clone(), card);
    }

    /// Discover an agent by name.
    pub fn discover(&self, name: &str) -> Option<AgentCard> {
        self.agents.get(name).map(|entry| entry.value().clone())
    }

    /// List all registered agent cards.
    pub fn list(&self) -> Vec<AgentCard> {
        self.agents
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Remove an agent from the registry. Returns `true` if the agent was found.
    pub fn remove(&self, name: &str) -> bool {
        self.agents.remove(name).is_some()
    }

    /// Generate our own agent card — the fight card we publish to the world.
    pub fn our_card(name: &str, url: &str, capabilities: Vec<String>) -> AgentCard {
        AgentCard {
            name: name.to_string(),
            description: format!("Punch Agent: {name}"),
            url: url.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities,
            input_modes: vec!["text".to_string(), "json".to_string()],
            output_modes: vec!["text".to_string(), "json".to_string()],
            authentication: Some(A2AAuth::None),
        }
    }
}

impl Default for A2ARegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card(name: &str) -> AgentCard {
        AgentCard {
            name: name.to_string(),
            description: format!("Test agent {name}"),
            url: format!("http://localhost:8080/{name}"),
            version: "0.1.0".to_string(),
            capabilities: vec!["code".to_string(), "search".to_string()],
            input_modes: vec!["text".to_string()],
            output_modes: vec!["text".to_string()],
            authentication: None,
        }
    }

    #[test]
    fn test_agent_card_creation() {
        let card = sample_card("alpha");
        assert_eq!(card.name, "alpha");
        assert_eq!(card.capabilities.len(), 2);
        assert!(card.authentication.is_none());
    }

    #[test]
    fn test_registry_register_and_discover() {
        let reg = A2ARegistry::new();
        reg.register(sample_card("boxer"));
        let found = reg.discover("boxer");
        assert!(found.is_some());
        assert_eq!(found.as_ref().map(|c| c.name.as_str()), Some("boxer"));
    }

    #[test]
    fn test_registry_list() {
        let reg = A2ARegistry::new();
        reg.register(sample_card("a"));
        reg.register(sample_card("b"));
        let list = reg.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_registry_remove() {
        let reg = A2ARegistry::new();
        reg.register(sample_card("temp"));
        assert!(reg.remove("temp"));
        assert!(reg.discover("temp").is_none());
    }

    #[test]
    fn test_registry_remove_nonexistent() {
        let reg = A2ARegistry::new();
        assert!(!reg.remove("ghost"));
    }

    #[test]
    fn test_task_status_transitions() {
        let now = Utc::now();
        let mut task = A2ATask {
            id: "task-1".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({"prompt": "hello"}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(task.status, A2ATaskStatus::Pending);

        task.status = A2ATaskStatus::Running;
        assert_eq!(task.status, A2ATaskStatus::Running);

        task.status = A2ATaskStatus::Completed;
        task.output = Some(serde_json::json!({"result": "done"}));
        assert_eq!(task.status, A2ATaskStatus::Completed);
        assert!(task.output.is_some());
    }

    #[test]
    fn test_our_card_generation() {
        let card = A2ARegistry::our_card(
            "punch-main",
            "http://localhost:3000",
            vec!["coordination".to_string()],
        );
        assert_eq!(card.name, "punch-main");
        assert_eq!(card.url, "http://localhost:3000");
        assert!(card.input_modes.contains(&"text".to_string()));
        assert!(card.output_modes.contains(&"json".to_string()));
    }

    #[test]
    fn test_registry_count() {
        let reg = A2ARegistry::new();
        assert_eq!(reg.list().len(), 0);
        reg.register(sample_card("one"));
        reg.register(sample_card("two"));
        reg.register(sample_card("three"));
        assert_eq!(reg.list().len(), 3);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let card = sample_card("serial");
        let json = serde_json::to_string(&card).expect("serialize");
        let deserialized: AgentCard = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.name, "serial");
        assert_eq!(deserialized.capabilities, card.capabilities);
    }

    #[test]
    fn test_unknown_agent_returns_none() {
        let reg = A2ARegistry::new();
        reg.register(sample_card("known"));
        assert!(reg.discover("unknown").is_none());
    }

    #[test]
    fn test_duplicate_registration_overwrites() {
        let reg = A2ARegistry::new();
        let mut card1 = sample_card("dup");
        card1.description = "first".to_string();
        reg.register(card1);

        let mut card2 = sample_card("dup");
        card2.description = "second".to_string();
        reg.register(card2);

        let found = reg.discover("dup").expect("should exist");
        assert_eq!(found.description, "second");
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn test_task_input_serialization() {
        let input = A2ATaskInput {
            prompt: "Summarize this code".to_string(),
            context: serde_json::Map::new(),
            mode: "text".to_string(),
        };
        let json = serde_json::to_string(&input).expect("serialize");
        let parsed: A2ATaskInput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.prompt, "Summarize this code");
        assert_eq!(parsed.mode, "text");
    }

    #[test]
    fn test_task_output_serialization() {
        let output = A2ATaskOutput {
            content: "Here is the summary".to_string(),
            data: Some(serde_json::json!({"tokens": 42})),
            mode: "text".to_string(),
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let parsed: A2ATaskOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.content, "Here is the summary");
        assert!(parsed.data.is_some());
    }

    #[test]
    fn test_task_input_default_mode() {
        let json = r#"{"prompt": "hello", "context": {}}"#;
        let input: A2ATaskInput = serde_json::from_str(json).expect("deserialize");
        assert_eq!(input.mode, "text");
    }

    #[test]
    fn test_task_output_optional_data() {
        let output = A2ATaskOutput {
            content: "done".to_string(),
            data: None,
            mode: "json".to_string(),
        };
        let json = serde_json::to_string(&output).expect("serialize");
        assert!(json.contains("\"data\":null"));
    }

    #[test]
    fn test_http_client_url_construction() {
        assert_eq!(
            HttpA2AClient::build_url("http://localhost:3000", "/.well-known/agent.json"),
            "http://localhost:3000/.well-known/agent.json"
        );
        assert_eq!(
            HttpA2AClient::build_url("http://localhost:3000/", "/a2a/tasks/send"),
            "http://localhost:3000/a2a/tasks/send"
        );
        assert_eq!(
            HttpA2AClient::build_url("https://agent.example.com", "/a2a/tasks/abc-123"),
            "https://agent.example.com/a2a/tasks/abc-123"
        );
    }

    #[test]
    fn test_http_client_creation() {
        let client = HttpA2AClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_custom_timeout() {
        let client = HttpA2AClient::with_timeout(Duration::from_secs(5));
        assert!(client.is_ok());
    }

    #[test]
    fn test_task_status_serialization_roundtrip() {
        let statuses = vec![
            A2ATaskStatus::Pending,
            A2ATaskStatus::Running,
            A2ATaskStatus::Completed,
            A2ATaskStatus::Failed("boom".to_string()),
            A2ATaskStatus::Cancelled,
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).expect("serialize");
            let parsed: A2ATaskStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_a2a_message_serialization() {
        let msg = A2AMessage {
            task_id: "t1".to_string(),
            role: "agent".to_string(),
            content: "Working on it".to_string(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: A2AMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.task_id, "t1");
        assert_eq!(parsed.role, "agent");
    }

    #[test]
    fn test_agent_card_with_auth() {
        let card = AgentCard {
            name: "secure-agent".to_string(),
            description: "An authenticated agent".to_string(),
            url: "https://secure.example.com".to_string(),
            version: "1.0.0".to_string(),
            capabilities: vec!["code".to_string()],
            input_modes: vec!["text".to_string()],
            output_modes: vec!["text".to_string()],
            authentication: Some(A2AAuth::Bearer("token123".to_string())),
        };
        let json = serde_json::to_string(&card).expect("serialize");
        let parsed: AgentCard = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed.authentication, Some(A2AAuth::Bearer(ref t)) if t == "token123"));
    }
}
