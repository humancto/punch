//! # punch-skills
//!
//! Skill/move system for the Punch Agent Combat System.
//!
//! Skills are bundles of tools, requirements, and domain-specific prompts
//! that can be loaded into a fighter to grant it new capabilities.

pub mod marketplace;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use punch_types::ToolDefinition;

pub use marketplace::{
    InstalledSkill, SkillListing, SkillMarketplace, SkillSource, builtin_skills,
};

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
    /// In the future this will read from the bundled/ directory. For now it
    /// initialises an empty registry.
    pub fn load_bundled() -> Self {
        info!("loading bundled skill manifests");
        Self::new()
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
