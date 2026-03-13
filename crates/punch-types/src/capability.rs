use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A capability that can be granted to a Fighter or Gorilla.
///
/// Capabilities follow a least-privilege model: agents only receive
/// the permissions they need, scoped to specific patterns where applicable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "scope")]
pub enum Capability {
    /// Read files matching the given glob pattern.
    FileRead(String),
    /// Write files matching the given glob pattern.
    FileWrite(String),
    /// Run shell commands matching the given pattern.
    ShellExec(String),
    /// Make network requests to the given host/pattern.
    Network(String),
    /// Access the memory subsystem.
    Memory,
    /// Access the knowledge graph.
    KnowledgeGraph,
    /// Control a browser instance.
    BrowserControl,
    /// Spawn new agents.
    AgentSpawn,
    /// Send messages to other agents.
    AgentMessage,
    /// Create and manage scheduled tasks.
    Schedule,
    /// Publish events to the event bus.
    EventPublish,
    /// Source control operations (git).
    SourceControl,
    /// Container operations (docker).
    Container,
    /// Data manipulation (JSON, YAML, regex).
    DataManipulation,
    /// Code analysis (search, symbols).
    CodeAnalysis,
    /// Archive operations (create, extract, list tar.gz).
    Archive,
    /// Template rendering operations.
    Template,
    /// Cryptographic hash operations.
    Crypto,
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileRead(g) => write!(f, "file_read({})", g),
            Self::FileWrite(g) => write!(f, "file_write({})", g),
            Self::ShellExec(p) => write!(f, "shell_exec({})", p),
            Self::Network(h) => write!(f, "network({})", h),
            Self::Memory => write!(f, "memory"),
            Self::KnowledgeGraph => write!(f, "knowledge_graph"),
            Self::BrowserControl => write!(f, "browser_control"),
            Self::AgentSpawn => write!(f, "agent_spawn"),
            Self::AgentMessage => write!(f, "agent_message"),
            Self::Schedule => write!(f, "schedule"),
            Self::EventPublish => write!(f, "event_publish"),
            Self::SourceControl => write!(f, "source_control"),
            Self::Container => write!(f, "container"),
            Self::DataManipulation => write!(f, "data_manipulation"),
            Self::CodeAnalysis => write!(f, "code_analysis"),
            Self::Archive => write!(f, "archive"),
            Self::Template => write!(f, "template"),
            Self::Crypto => write!(f, "crypto"),
        }
    }
}

/// A record of a capability grant to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityGrant {
    /// Unique identifier for this grant.
    pub id: Uuid,
    /// The capability that was granted.
    pub capability: Capability,
    /// Who or what granted this capability.
    pub granted_by: String,
    /// When the grant was issued.
    pub granted_at: DateTime<Utc>,
    /// Optional expiration time.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Check whether a granted capability satisfies a required capability.
///
/// Scope-less capabilities match by variant equality. For scoped capabilities,
/// the granted pattern is matched against the required pattern using glob matching.
pub fn capability_matches(granted: &Capability, required: &Capability) -> bool {
    match (granted, required) {
        (Capability::FileRead(granted_glob), Capability::FileRead(required_path)) => {
            glob_matches(granted_glob, required_path)
        }
        (Capability::FileWrite(granted_glob), Capability::FileWrite(required_path)) => {
            glob_matches(granted_glob, required_path)
        }
        (Capability::ShellExec(granted_pat), Capability::ShellExec(required_cmd)) => {
            pattern_matches(granted_pat, required_cmd)
        }
        (Capability::Network(granted_host), Capability::Network(required_host)) => {
            host_matches(granted_host, required_host)
        }
        (Capability::Memory, Capability::Memory) => true,
        (Capability::KnowledgeGraph, Capability::KnowledgeGraph) => true,
        (Capability::BrowserControl, Capability::BrowserControl) => true,
        (Capability::AgentSpawn, Capability::AgentSpawn) => true,
        (Capability::AgentMessage, Capability::AgentMessage) => true,
        (Capability::Schedule, Capability::Schedule) => true,
        (Capability::EventPublish, Capability::EventPublish) => true,
        (Capability::SourceControl, Capability::SourceControl) => true,
        (Capability::Container, Capability::Container) => true,
        (Capability::DataManipulation, Capability::DataManipulation) => true,
        (Capability::CodeAnalysis, Capability::CodeAnalysis) => true,
        (Capability::Archive, Capability::Archive) => true,
        (Capability::Template, Capability::Template) => true,
        (Capability::Crypto, Capability::Crypto) => true,
        _ => false,
    }
}

/// Match a glob pattern against a path string.
fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern == "**" || pattern == "**/*" {
        return true;
    }
    glob::Pattern::new(pattern)
        .map(|p| p.matches(path))
        .unwrap_or(false)
}

/// Match a command pattern against a required command.
fn pattern_matches(pattern: &str, command: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    glob::Pattern::new(pattern)
        .map(|p| p.matches(command))
        .unwrap_or(false)
}

/// Match a host pattern against a required host.
fn host_matches(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host == suffix || host.ends_with(&format!(".{}", suffix));
    }
    pattern == host
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_file_read() {
        let granted = Capability::FileRead("src/**/*.rs".to_string());
        let required = Capability::FileRead("src/main.rs".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_wildcard_grants_all() {
        let granted = Capability::FileRead("**".to_string());
        let required = Capability::FileRead("anything/at/all.txt".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_glob_no_match() {
        let granted = Capability::FileRead("src/**/*.rs".to_string());
        let required = Capability::FileRead("tests/data.json".to_string());
        assert!(!capability_matches(&granted, &required));
    }

    #[test]
    fn test_variant_mismatch() {
        let granted = Capability::FileRead("**".to_string());
        let required = Capability::FileWrite("foo.txt".to_string());
        assert!(!capability_matches(&granted, &required));
    }

    #[test]
    fn test_scopeless_capabilities() {
        assert!(capability_matches(&Capability::Memory, &Capability::Memory));
        assert!(!capability_matches(
            &Capability::Memory,
            &Capability::Schedule
        ));
    }

    #[test]
    fn test_wildcard_host() {
        let granted = Capability::Network("*.example.com".to_string());
        let required = Capability::Network("api.example.com".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_exact_host() {
        let granted = Capability::Network("api.example.com".to_string());
        let required = Capability::Network("api.example.com".to_string());
        assert!(capability_matches(&granted, &required));

        let other = Capability::Network("other.example.com".to_string());
        assert!(!capability_matches(&granted, &other));
    }
}
