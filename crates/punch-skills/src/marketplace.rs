//! # Skill Marketplace
//!
//! A marketplace for sharing and discovering agent skills (special moves).
//! Fighters can browse the marketplace to find new tools, install them into
//! their loadout, and rate them after use.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use punch_types::error::{PunchError, PunchResult};
use punch_types::{ToolCategory, ToolDefinition};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Where a skill comes from — its origin story.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillSource {
    /// Ships with Punch out of the box.
    Builtin,
    /// Loaded from a local path on disk.
    Local(PathBuf),
    /// Fetched from a remote URL.
    Remote(String),
    /// Provided by a plugin.
    Plugin(Uuid),
}

/// A skill listing in the marketplace — like a fighter's move on the roster card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillListing {
    /// Unique identifier.
    pub id: Uuid,
    /// Human-readable name of the skill.
    pub name: String,
    /// Description of what this skill does.
    pub description: String,
    /// Author or team that created the skill.
    pub author: String,
    /// Semantic version.
    pub version: String,
    /// Category (e.g. "filesystem", "web", "agent", "code").
    pub category: String,
    /// Searchable tags.
    pub tags: Vec<String>,
    /// The actual tool definitions this skill provides.
    pub tool_definitions: Vec<ToolDefinition>,
    /// Number of times this skill has been installed.
    pub install_count: u64,
    /// Average user rating (0.0–5.0).
    pub rating: f64,
    /// When the skill was published.
    pub published_at: DateTime<Utc>,
    /// Where the skill comes from.
    pub source: SkillSource,
}

/// A skill that has been installed into the fighter's loadout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    /// The ID of the skill listing this was installed from.
    pub skill_id: Uuid,
    /// When it was installed.
    pub installed_at: DateTime<Utc>,
    /// The tools this skill provides.
    pub tools: Vec<ToolDefinition>,
    /// Whether the skill is currently enabled.
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Marketplace
// ---------------------------------------------------------------------------

/// The skill marketplace — a bazaar of special moves that fighters can equip.
pub struct SkillMarketplace {
    skills: DashMap<Uuid, SkillListing>,
    installed: DashMap<Uuid, InstalledSkill>,
}

impl SkillMarketplace {
    /// Create a new empty marketplace.
    pub fn new() -> Self {
        Self {
            skills: DashMap::new(),
            installed: DashMap::new(),
        }
    }

    /// Publish a skill listing to the marketplace. Returns the skill's ID.
    pub fn publish(&self, listing: SkillListing) -> Uuid {
        let id = listing.id;
        self.skills.insert(id, listing);
        id
    }

    /// Search for skills whose name, description, or tags contain the query.
    pub fn search(&self, query: &str) -> Vec<SkillListing> {
        let q = query.to_lowercase();
        self.skills
            .iter()
            .filter(|entry| {
                let s = entry.value();
                s.name.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Search for skills by category.
    pub fn search_by_category(&self, category: &str) -> Vec<SkillListing> {
        let cat = category.to_lowercase();
        self.skills
            .iter()
            .filter(|entry| entry.value().category.to_lowercase() == cat)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get a skill listing by ID.
    pub fn get(&self, id: &Uuid) -> Option<SkillListing> {
        self.skills.get(id).map(|entry| entry.value().clone())
    }

    /// Install a skill from the marketplace.
    pub fn install(&self, id: &Uuid) -> PunchResult<InstalledSkill> {
        let listing = self.skills.get(id).ok_or_else(|| {
            PunchError::ToolNotFound(format!("skill {id} not found in marketplace"))
        })?;

        let installed = InstalledSkill {
            skill_id: *id,
            installed_at: Utc::now(),
            tools: listing.tool_definitions.clone(),
            enabled: true,
        };

        // Bump install count
        drop(listing);
        if let Some(mut entry) = self.skills.get_mut(id) {
            entry.value_mut().install_count += 1;
        }

        self.installed.insert(*id, installed.clone());
        Ok(installed)
    }

    /// Uninstall a skill.
    pub fn uninstall(&self, id: &Uuid) -> PunchResult<()> {
        self.installed
            .remove(id)
            .ok_or_else(|| PunchError::ToolNotFound(format!("skill {id} is not installed")))?;
        Ok(())
    }

    /// List all installed skills.
    pub fn installed_skills(&self) -> Vec<InstalledSkill> {
        self.installed
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Check if a skill is currently installed.
    pub fn is_installed(&self, id: &Uuid) -> bool {
        self.installed.contains_key(id)
    }

    /// Update the rating for a skill.
    pub fn update_rating(&self, id: &Uuid, rating: f64) {
        if let Some(mut entry) = self.skills.get_mut(id) {
            entry.value_mut().rating = rating;
        }
    }
}

impl Default for SkillMarketplace {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in skills
// ---------------------------------------------------------------------------

/// Helper to build a simple tool definition.
fn tool(name: &str, description: &str, category: ToolCategory) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        category,
    }
}

/// Returns listings for all built-in skills that ship with Punch.
pub fn builtin_skills() -> Vec<SkillListing> {
    let now = Utc::now();

    vec![
        SkillListing {
            id: Uuid::new_v4(),
            name: "Filesystem Tools".to_string(),
            description: "Read, write, and list files on the local filesystem.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "filesystem".to_string(),
            tags: vec!["io".to_string(), "files".to_string(), "builtin".to_string()],
            tool_definitions: vec![
                tool(
                    "file_read",
                    "Read the contents of a file",
                    ToolCategory::FileSystem,
                ),
                tool(
                    "file_write",
                    "Write content to a file",
                    ToolCategory::FileSystem,
                ),
                tool(
                    "file_list",
                    "List files in a directory",
                    ToolCategory::FileSystem,
                ),
            ],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Shell Tools".to_string(),
            description: "Execute shell commands.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "shell".to_string(),
            tags: vec![
                "exec".to_string(),
                "command".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![tool(
                "shell_exec",
                "Execute a shell command",
                ToolCategory::Shell,
            )],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Web Tools".to_string(),
            description: "Search the web and fetch URLs.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "web".to_string(),
            tags: vec![
                "http".to_string(),
                "search".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![
                tool(
                    "web_search",
                    "Search the web for information",
                    ToolCategory::Web,
                ),
                tool("web_fetch", "Fetch content from a URL", ToolCategory::Web),
            ],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Memory Tools".to_string(),
            description: "Store and recall information from memory.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "memory".to_string(),
            tags: vec![
                "recall".to_string(),
                "store".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![
                tool(
                    "memory_store",
                    "Store a value in memory",
                    ToolCategory::Memory,
                ),
                tool(
                    "memory_recall",
                    "Recall a value from memory",
                    ToolCategory::Memory,
                ),
            ],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Knowledge Graph".to_string(),
            description: "Build and query a knowledge graph of entities and relations.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "knowledge".to_string(),
            tags: vec![
                "graph".to_string(),
                "entities".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![
                tool(
                    "knowledge_add_entity",
                    "Add an entity to the knowledge graph",
                    ToolCategory::Knowledge,
                ),
                tool(
                    "knowledge_add_relation",
                    "Add a relation between entities",
                    ToolCategory::Knowledge,
                ),
                tool(
                    "knowledge_query",
                    "Query the knowledge graph",
                    ToolCategory::Knowledge,
                ),
            ],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Agent Coordination".to_string(),
            description: "Spawn, message, and list other agents in the ring.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "agent".to_string(),
            tags: vec![
                "multi-agent".to_string(),
                "coordination".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![
                tool("agent_spawn", "Spawn a new agent", ToolCategory::Agent),
                tool(
                    "agent_message",
                    "Send a message to another agent",
                    ToolCategory::Agent,
                ),
                tool("agent_list", "List all active agents", ToolCategory::Agent),
            ],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Browser Tools".to_string(),
            description: "Navigate and interact with web pages in a browser.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "browser".to_string(),
            tags: vec![
                "web".to_string(),
                "scrape".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![tool(
                "browser_navigate",
                "Navigate to a URL in the browser",
                ToolCategory::Browser,
            )],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
        SkillListing {
            id: Uuid::new_v4(),
            name: "Patch Tools".to_string(),
            description: "Apply unified diffs and patches to files.".to_string(),
            author: "Punch Team".to_string(),
            version: "0.1.0".to_string(),
            category: "code".to_string(),
            tags: vec![
                "diff".to_string(),
                "patch".to_string(),
                "builtin".to_string(),
            ],
            tool_definitions: vec![tool(
                "patch_apply",
                "Apply a unified diff patch",
                ToolCategory::FileSystem,
            )],
            install_count: 0,
            rating: 0.0,
            published_at: now,
            source: SkillSource::Builtin,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_listing(name: &str, category: &str) -> SkillListing {
        SkillListing {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: format!("A skill for {name}"),
            author: "tester".to_string(),
            version: "1.0.0".to_string(),
            category: category.to_string(),
            tags: vec!["test".to_string(), category.to_string()],
            tool_definitions: vec![tool("test_tool", "a test tool", ToolCategory::Shell)],
            install_count: 0,
            rating: 0.0,
            published_at: Utc::now(),
            source: SkillSource::Builtin,
        }
    }

    #[test]
    fn test_publish_skill() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("puncher", "agent");
        let id = listing.id;
        let returned = mp.publish(listing);
        assert_eq!(returned, id);
        assert!(mp.get(&id).is_some());
    }

    #[test]
    fn test_search_by_name() {
        let mp = SkillMarketplace::new();
        mp.publish(sample_listing("Filesystem Tools", "filesystem"));
        mp.publish(sample_listing("Web Tools", "web"));

        let results = mp.search("filesystem");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Filesystem Tools");
    }

    #[test]
    fn test_search_by_category() {
        let mp = SkillMarketplace::new();
        mp.publish(sample_listing("Tool A", "web"));
        mp.publish(sample_listing("Tool B", "web"));
        mp.publish(sample_listing("Tool C", "agent"));

        let results = mp.search_by_category("web");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_by_tag() {
        let mp = SkillMarketplace::new();
        let mut listing = sample_listing("Tagged", "misc");
        listing.tags.push("special_move".to_string());
        mp.publish(listing);

        let results = mp.search("special_move");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Tagged");
    }

    #[test]
    fn test_install_skill() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("installable", "agent");
        let id = listing.id;
        mp.publish(listing);

        let installed = mp.install(&id).expect("install should succeed");
        assert_eq!(installed.skill_id, id);
        assert!(installed.enabled);
        assert!(!installed.tools.is_empty());

        // Install count should be bumped
        let updated = mp.get(&id).expect("should exist");
        assert_eq!(updated.install_count, 1);
    }

    #[test]
    fn test_uninstall_skill() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("removable", "agent");
        let id = listing.id;
        mp.publish(listing);
        mp.install(&id).expect("install");

        assert!(mp.is_installed(&id));
        mp.uninstall(&id).expect("uninstall");
        assert!(!mp.is_installed(&id));
    }

    #[test]
    fn test_is_installed_check() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("checker", "agent");
        let id = listing.id;
        mp.publish(listing);

        assert!(!mp.is_installed(&id));
        mp.install(&id).expect("install");
        assert!(mp.is_installed(&id));
    }

    #[test]
    fn test_update_rating() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("rated", "agent");
        let id = listing.id;
        mp.publish(listing);

        mp.update_rating(&id, 4.5);
        let updated = mp.get(&id).expect("should exist");
        assert!((updated.rating - 4.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_builtin_skills_populated() {
        let skills = builtin_skills();
        assert!(
            skills.len() >= 8,
            "expected at least 8 builtin skills, got {}",
            skills.len()
        );

        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Filesystem Tools"));
        assert!(names.contains(&"Shell Tools"));
        assert!(names.contains(&"Web Tools"));
        assert!(names.contains(&"Memory Tools"));
        assert!(names.contains(&"Knowledge Graph"));
        assert!(names.contains(&"Agent Coordination"));
        assert!(names.contains(&"Browser Tools"));
        assert!(names.contains(&"Patch Tools"));
    }

    #[test]
    fn test_get_by_id() {
        let mp = SkillMarketplace::new();
        let listing = sample_listing("findme", "code");
        let id = listing.id;
        mp.publish(listing);

        let found = mp.get(&id);
        assert!(found.is_some());
        assert_eq!(found.as_ref().map(|s| s.name.as_str()), Some("findme"));

        let missing = mp.get(&Uuid::new_v4());
        assert!(missing.is_none());
    }
}
