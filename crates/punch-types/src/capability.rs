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
    /// Invoke loaded WASM plugins (imported techniques).
    PluginInvoke,
    /// Delegate tasks to remote A2A agents.
    A2ADelegate,
    /// Access MCP servers matching the given name pattern (e.g., "*" for all, "github" for specific).
    McpAccess(String),
    /// Send proactive notifications to connected channels (Telegram, Slack, Discord, etc.).
    ChannelNotify,
    /// Self-configuration: modify own creed, heartbeats, and install skill packs.
    SelfConfig,
    /// System-level automation: screenshots, screen recording permission required.
    SystemAutomation,
    /// UI automation scoped to a specific app (accessibility tree interaction).
    /// Use "*" to grant access to all apps.
    UiAutomation(String),
    /// Deep app integration scoped to a specific app (OCR, app-specific APIs).
    /// Use "*" to grant access to all apps.
    AppIntegration(String),
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
            Self::PluginInvoke => write!(f, "plugin_invoke"),
            Self::A2ADelegate => write!(f, "a2a_delegate"),
            Self::McpAccess(p) => write!(f, "mcp_access({})", p),
            Self::ChannelNotify => write!(f, "channel_notify"),
            Self::SelfConfig => write!(f, "self_config"),
            Self::SystemAutomation => write!(f, "system_automation"),
            Self::UiAutomation(app) => write!(f, "ui_automation({})", app),
            Self::AppIntegration(app) => write!(f, "app_integration({})", app),
        }
    }
}

impl Capability {
    /// Returns a full-access capability set with wildcard scopes.
    ///
    /// This is the default for user-facing fighters — they should be able to
    /// use any tool the LLM decides is appropriate. Restrict capabilities only
    /// when deploying sandboxed or multi-tenant agents.
    pub fn full_access() -> Vec<Capability> {
        vec![
            Capability::FileRead("**".to_string()),
            Capability::FileWrite("**".to_string()),
            Capability::ShellExec("*".to_string()),
            Capability::Network("*".to_string()),
            Capability::Memory,
            Capability::KnowledgeGraph,
            Capability::BrowserControl,
            Capability::AgentSpawn,
            Capability::AgentMessage,
            Capability::Schedule,
            Capability::EventPublish,
            Capability::SourceControl,
            Capability::Container,
            Capability::DataManipulation,
            Capability::CodeAnalysis,
            Capability::Archive,
            Capability::Template,
            Capability::Crypto,
            Capability::PluginInvoke,
            Capability::A2ADelegate,
            Capability::McpAccess("*".to_string()),
            Capability::ChannelNotify,
            Capability::SelfConfig,
            Capability::SystemAutomation,
            Capability::UiAutomation("*".to_string()),
            Capability::AppIntegration("*".to_string()),
        ]
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
        (Capability::PluginInvoke, Capability::PluginInvoke) => true,
        (Capability::A2ADelegate, Capability::A2ADelegate) => true,
        (Capability::McpAccess(granted_pat), Capability::McpAccess(required_name)) => {
            pattern_matches(granted_pat, required_name)
        }
        (Capability::ChannelNotify, Capability::ChannelNotify) => true,
        (Capability::SelfConfig, Capability::SelfConfig) => true,
        (Capability::SystemAutomation, Capability::SystemAutomation) => true,
        (Capability::UiAutomation(granted_app), Capability::UiAutomation(required_app)) => {
            pattern_matches(granted_app, required_app)
        }
        (Capability::AppIntegration(granted_app), Capability::AppIntegration(required_app)) => {
            pattern_matches(granted_app, required_app)
        }
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

    #[test]
    fn test_capability_display_all_scoped() {
        assert_eq!(
            Capability::FileRead("src/*.rs".to_string()).to_string(),
            "file_read(src/*.rs)"
        );
        assert_eq!(
            Capability::FileWrite("out/**".to_string()).to_string(),
            "file_write(out/**)"
        );
        assert_eq!(
            Capability::ShellExec("ls*".to_string()).to_string(),
            "shell_exec(ls*)"
        );
        assert_eq!(
            Capability::Network("*.example.com".to_string()).to_string(),
            "network(*.example.com)"
        );
    }

    #[test]
    fn test_mcp_access_wildcard() {
        let granted = Capability::McpAccess("*".to_string());
        let required = Capability::McpAccess("github".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_mcp_access_exact() {
        let granted = Capability::McpAccess("github".to_string());
        assert!(capability_matches(
            &granted,
            &Capability::McpAccess("github".to_string())
        ));
        assert!(!capability_matches(
            &granted,
            &Capability::McpAccess("slack".to_string())
        ));
    }

    #[test]
    fn test_mcp_access_display() {
        assert_eq!(
            Capability::McpAccess("github".to_string()).to_string(),
            "mcp_access(github)"
        );
    }

    #[test]
    fn test_capability_display_all_scopeless() {
        assert_eq!(Capability::Memory.to_string(), "memory");
        assert_eq!(Capability::KnowledgeGraph.to_string(), "knowledge_graph");
        assert_eq!(Capability::BrowserControl.to_string(), "browser_control");
        assert_eq!(Capability::AgentSpawn.to_string(), "agent_spawn");
        assert_eq!(Capability::AgentMessage.to_string(), "agent_message");
        assert_eq!(Capability::Schedule.to_string(), "schedule");
        assert_eq!(Capability::EventPublish.to_string(), "event_publish");
        assert_eq!(Capability::SourceControl.to_string(), "source_control");
        assert_eq!(Capability::Container.to_string(), "container");
        assert_eq!(
            Capability::DataManipulation.to_string(),
            "data_manipulation"
        );
        assert_eq!(Capability::CodeAnalysis.to_string(), "code_analysis");
        assert_eq!(Capability::Archive.to_string(), "archive");
        assert_eq!(Capability::Template.to_string(), "template");
        assert_eq!(Capability::Crypto.to_string(), "crypto");
        assert_eq!(Capability::A2ADelegate.to_string(), "a2a_delegate");
        assert_eq!(Capability::PluginInvoke.to_string(), "plugin_invoke");
        assert_eq!(Capability::SelfConfig.to_string(), "self_config");
        assert_eq!(
            Capability::SystemAutomation.to_string(),
            "system_automation"
        );
    }

    #[test]
    fn test_all_scopeless_capability_matches() {
        let scopeless = vec![
            Capability::Memory,
            Capability::KnowledgeGraph,
            Capability::BrowserControl,
            Capability::AgentSpawn,
            Capability::AgentMessage,
            Capability::Schedule,
            Capability::EventPublish,
            Capability::SourceControl,
            Capability::Container,
            Capability::DataManipulation,
            Capability::CodeAnalysis,
            Capability::Archive,
            Capability::Template,
            Capability::Crypto,
            Capability::A2ADelegate,
            Capability::PluginInvoke,
            Capability::SelfConfig,
            Capability::SystemAutomation,
        ];
        for cap in &scopeless {
            assert!(capability_matches(cap, cap), "{} should match itself", cap);
        }
        // Scoped MCP access should match itself
        let mcp = Capability::McpAccess("test".to_string());
        assert!(capability_matches(&mcp, &mcp));
        // Cross-variant should not match
        assert!(!capability_matches(
            &Capability::Memory,
            &Capability::Schedule
        ));
        assert!(!capability_matches(
            &Capability::Archive,
            &Capability::Template
        ));
    }

    #[test]
    fn test_capability_serde_roundtrip() {
        let caps = vec![
            Capability::FileRead("**/*.rs".to_string()),
            Capability::FileWrite("out/**".to_string()),
            Capability::ShellExec("*".to_string()),
            Capability::Network("*.api.com".to_string()),
            Capability::Memory,
            Capability::BrowserControl,
            Capability::Crypto,
            Capability::McpAccess("*".to_string()),
        ];
        for cap in &caps {
            let json = serde_json::to_string(cap).expect("serialize");
            let deser: Capability = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, cap);
        }
    }

    #[test]
    fn test_glob_matches_star_star_slash_star() {
        let granted = Capability::FileRead("**/*".to_string());
        let required = Capability::FileRead("deep/nested/file.txt".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_shell_wildcard_grants_all() {
        let granted = Capability::ShellExec("*".to_string());
        let required = Capability::ShellExec("rm -rf /".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_network_wildcard_grants_all() {
        let granted = Capability::Network("*".to_string());
        let required = Capability::Network("any.host.com".to_string());
        assert!(capability_matches(&granted, &required));
    }

    #[test]
    fn test_subdomain_wildcard_host() {
        let granted = Capability::Network("*.example.com".to_string());
        // Direct match of the suffix
        assert!(capability_matches(
            &granted,
            &Capability::Network("example.com".to_string())
        ));
        // Deep subdomain
        assert!(capability_matches(
            &granted,
            &Capability::Network("deep.sub.example.com".to_string())
        ));
    }

    #[test]
    fn test_capability_grant_construction() {
        let grant = CapabilityGrant {
            id: Uuid::new_v4(),
            capability: Capability::Memory,
            granted_by: "admin".to_string(),
            granted_at: chrono::Utc::now(),
            expires_at: None,
        };
        assert_eq!(grant.granted_by, "admin");
        assert!(grant.expires_at.is_none());
    }

    #[test]
    fn test_ui_automation_scoped() {
        let granted = Capability::UiAutomation("*".to_string());
        let required = Capability::UiAutomation("Messages".to_string());
        assert!(capability_matches(&granted, &required));

        let specific = Capability::UiAutomation("Safari".to_string());
        assert!(capability_matches(
            &specific,
            &Capability::UiAutomation("Safari".to_string())
        ));
        assert!(!capability_matches(
            &specific,
            &Capability::UiAutomation("Messages".to_string())
        ));
    }

    #[test]
    fn test_app_integration_scoped() {
        let granted = Capability::AppIntegration("*".to_string());
        let required = Capability::AppIntegration("Messages".to_string());
        assert!(capability_matches(&granted, &required));

        let specific = Capability::AppIntegration("Safari".to_string());
        assert!(!capability_matches(
            &specific,
            &Capability::AppIntegration("Finder".to_string())
        ));
    }

    #[test]
    fn test_automation_display() {
        assert_eq!(
            Capability::UiAutomation("Safari".to_string()).to_string(),
            "ui_automation(Safari)"
        );
        assert_eq!(
            Capability::AppIntegration("*".to_string()).to_string(),
            "app_integration(*)"
        );
    }

    #[test]
    fn test_automation_cross_variant_no_match() {
        assert!(!capability_matches(
            &Capability::SystemAutomation,
            &Capability::UiAutomation("*".to_string())
        ));
        assert!(!capability_matches(
            &Capability::UiAutomation("*".to_string()),
            &Capability::AppIntegration("*".to_string())
        ));
    }

    #[test]
    fn test_capability_grant_with_expiry() {
        let grant = CapabilityGrant {
            id: Uuid::new_v4(),
            capability: Capability::Network("*".to_string()),
            granted_by: "system".to_string(),
            granted_at: chrono::Utc::now(),
            expires_at: Some(chrono::Utc::now()),
        };
        assert!(grant.expires_at.is_some());
    }
}
