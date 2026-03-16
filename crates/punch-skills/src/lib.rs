//! # punch-skills
//!
//! Skill/move system for the Punch Agent Combat System.
//!
//! Skills are bundles of tools, requirements, and domain-specific prompts
//! that can be loaded into a fighter to grant it new capabilities.

pub mod client;
pub mod loader;
pub mod lockfile;
pub mod marketplace;
pub mod publisher;
pub mod registry;
pub mod scanner;
pub mod verifier;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use punch_types::ToolDefinition;

pub use client::IndexClient;
pub use loader::{
    LoadedSkill, SkillFrontmatter, SkillPrecedence, load_all_skills,
    load_all_skills_with_marketplace, load_skill_from_dir, load_skills_from_dir, parse_skill_md,
    render_skills_prompt,
};
pub use lockfile::{LockedMove, MoveLockfile};
pub use marketplace::{
    InstalledSkill, SkillListing, SkillMarketplace, SkillSource, builtin_skills,
};
pub use registry::{IndexEntry, IndexMeta, ScanFinding, ScanVerdict};
pub use scanner::SkillScanner;
pub use verifier::{verify_and_scan, verify_checksum, verify_signature};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// The kind of requirement a skill needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    /// A binary must be available on PATH.
    Binary,
    /// An environment variable must be set.
    EnvVar,
    /// An API key must be configured.
    ApiKey,
}

/// A single requirement for a skill to function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRequirement {
    /// Human-readable name of the requirement.
    pub name: String,
    /// What kind of requirement this is.
    pub kind: RequirementKind,
    /// Optional command to run to check if the requirement is met.
    pub check_command: Option<String>,
}

/// A skill manifest describes a loadable skill package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique name of the skill.
    pub name: String,
    /// Version string (semver).
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Author or team.
    pub author: String,
    /// Tools this skill provides.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Requirements that must be met for the skill to work.
    #[serde(default)]
    pub requirements: Vec<SkillRequirement>,
    /// Domain expertise text injected into the system prompt.
    #[serde(default)]
    pub skill_prompt: String,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of available skills.
pub struct SkillRegistry {
    skills: HashMap<String, SkillManifest>,
}

impl SkillRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Load bundled skill manifests.
    ///
    /// Populates the registry with all built-in skills that ship with Punch.
    /// Each skill is converted from its marketplace listing into a manifest
    /// and registered.
    pub fn load_bundled() -> Self {
        info!("loading bundled skill manifests");
        let mut registry = Self::new();

        for listing in builtin_skills() {
            let manifest = SkillManifest {
                name: listing.name,
                version: listing.version,
                description: listing.description,
                author: listing.author,
                tools: listing.tool_definitions,
                requirements: Vec::new(),
                skill_prompt: String::new(),
            };
            registry.register(manifest);
        }

        info!(count = registry.skills.len(), "bundled skills loaded");
        registry
    }

    /// Register a skill manifest.
    pub fn register(&mut self, manifest: SkillManifest) {
        info!(skill = %manifest.name, "registering skill");
        self.skills.insert(manifest.name.clone(), manifest);
    }

    /// Get a skill by name.
    pub fn get_skill(&self, name: &str) -> Option<&SkillManifest> {
        self.skills.get(name)
    }

    /// List all registered skill names.
    pub fn list_skills(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// Search for skills whose name or description contains the query string.
    pub fn search_skills(&self, query: &str) -> Vec<&SkillManifest> {
        let query_lower = query.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
            })
            .collect()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::ToolCategory;

    fn sample_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("A skill called {name}"),
            author: "tester".to_string(),
            tools: vec![],
            requirements: vec![],
            skill_prompt: String::new(),
        }
    }

    #[test]
    fn test_registry_new_empty() {
        let registry = SkillRegistry::new();
        assert!(registry.list_skills().is_empty());
    }

    #[test]
    fn test_registry_default_empty() {
        let registry = SkillRegistry::default();
        assert!(registry.list_skills().is_empty());
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = SkillRegistry::new();
        registry.register(sample_manifest("test-skill"));

        let skill = registry.get_skill("test-skill");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().name, "test-skill");
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = SkillRegistry::new();
        assert!(registry.get_skill("missing").is_none());
    }

    #[test]
    fn test_list_skills() {
        let mut registry = SkillRegistry::new();
        registry.register(sample_manifest("alpha"));
        registry.register(sample_manifest("beta"));

        let mut names = registry.list_skills();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_register_overwrites() {
        let mut registry = SkillRegistry::new();
        let mut m1 = sample_manifest("skill");
        m1.description = "original".to_string();
        registry.register(m1);

        let mut m2 = sample_manifest("skill");
        m2.description = "updated".to_string();
        registry.register(m2);

        let skill = registry.get_skill("skill").unwrap();
        assert_eq!(skill.description, "updated");
        assert_eq!(registry.list_skills().len(), 1);
    }

    #[test]
    fn test_search_by_name() {
        let mut registry = SkillRegistry::new();
        registry.register(sample_manifest("filesystem-tools"));
        registry.register(sample_manifest("web-tools"));
        registry.register(sample_manifest("shell-exec"));

        let results = registry.search_skills("tool");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_by_description() {
        let mut registry = SkillRegistry::new();
        let mut m = sample_manifest("custom");
        m.description = "Handles HTTP requests".to_string();
        registry.register(m);

        let results = registry.search_skills("http");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "custom");
    }

    #[test]
    fn test_search_case_insensitive() {
        let mut registry = SkillRegistry::new();
        registry.register(sample_manifest("FileSystem"));

        let results = registry.search_skills("filesystem");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_no_match() {
        let mut registry = SkillRegistry::new();
        registry.register(sample_manifest("alpha"));

        let results = registry.search_skills("zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_skill_manifest_serde() {
        let manifest = SkillManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            description: "desc".to_string(),
            author: "author".to_string(),
            tools: vec![ToolDefinition {
                name: "my_tool".to_string(),
                description: "a tool".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                category: ToolCategory::Shell,
            }],
            requirements: vec![SkillRequirement {
                name: "git".to_string(),
                kind: RequirementKind::Binary,
                check_command: Some("git --version".to_string()),
            }],
            skill_prompt: "You are a test skill.".to_string(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let restored: SkillManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert_eq!(restored.tools.len(), 1);
        assert_eq!(restored.requirements.len(), 1);
    }

    #[test]
    fn test_requirement_kind_serde() {
        let kinds = vec![
            RequirementKind::Binary,
            RequirementKind::EnvVar,
            RequirementKind::ApiKey,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let restored: RequirementKind = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, kind);
        }
    }

    #[test]
    fn test_load_bundled_returns_populated() {
        let registry = SkillRegistry::load_bundled();
        let skills = registry.list_skills();
        assert!(
            skills.len() >= 8,
            "expected at least 8 bundled skills, got {}",
            skills.len()
        );
        assert!(registry.get_skill("Filesystem Tools").is_some());
        assert!(registry.get_skill("Shell Tools").is_some());
        assert!(registry.get_skill("Web Tools").is_some());
        assert!(registry.get_skill("Memory Tools").is_some());
        assert!(registry.get_skill("Knowledge Graph").is_some());
        assert!(registry.get_skill("Agent Coordination").is_some());
        assert!(registry.get_skill("Browser Tools").is_some());
        assert!(registry.get_skill("Patch Tools").is_some());
    }

    #[test]
    fn test_load_bundled_skills_have_descriptions() {
        let registry = SkillRegistry::load_bundled();
        for name in registry.list_skills() {
            let skill = registry.get_skill(&name).unwrap();
            assert!(
                !skill.description.is_empty(),
                "skill '{}' should have a non-empty description",
                name
            );
        }
    }

    #[test]
    fn test_load_bundled_skills_have_tools() {
        let registry = SkillRegistry::load_bundled();
        for name in registry.list_skills() {
            let skill = registry.get_skill(&name).unwrap();
            assert!(
                !skill.tools.is_empty(),
                "skill '{}' should have at least one tool",
                name
            );
        }
    }

    #[test]
    fn test_load_bundled_skills_have_valid_schemas() {
        let registry = SkillRegistry::load_bundled();
        for name in registry.list_skills() {
            let skill = registry.get_skill(&name).unwrap();
            for tool in &skill.tools {
                assert!(
                    tool.input_schema.is_object(),
                    "tool '{}' in skill '{}' should have an object input schema",
                    tool.name,
                    name
                );
                assert!(
                    tool.input_schema.get("type").is_some(),
                    "tool '{}' in skill '{}' should have a 'type' field in schema",
                    tool.name,
                    name
                );
            }
        }
    }

    #[test]
    fn test_load_bundled_skills_categories_assigned() {
        let registry = SkillRegistry::load_bundled();
        for name in registry.list_skills() {
            let skill = registry.get_skill(&name).unwrap();
            for tool in &skill.tools {
                // Just verify the category is accessible (no panic).
                let _ = format!("{:?}", tool.category);
            }
        }
    }
}
