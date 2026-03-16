//! Markdown-based skill loader.
//!
//! Skills are folders containing a SKILL.md file with YAML frontmatter.
//! This is the Punch equivalent of OpenClaw's skill system — the barrier
//! to creating a skill is writing a markdown file.
//!
//! ## SKILL.md format
//!
//! ```markdown
//! ---
//! name: code-reviewer
//! version: 1.0.0
//! description: Expert code review with security and performance analysis
//! author: HumanCTO
//! category: code_analysis
//! tags: [code, review, security, quality]
//! tools: [file_read, file_list, code_search, git_diff, git_log]
//! requires:
//!   - name: git
//!     kind: binary
//! ---
//!
//! # Code Reviewer
//!
//! You are an expert code reviewer. When reviewing code:
//!
//! 1. Check for security vulnerabilities (OWASP Top 10)
//! 2. Assess performance implications
//! 3. Verify error handling completeness
//! ...
//! ```
//!
//! ## Precedence (highest to lowest)
//!
//! 1. Workspace `./skills/` — project-specific skills
//! 2. User `~/.punch/skills/` — personal global skills
//! 3. Bundled skills — shipped with Punch

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use punch_types::PunchResult;

use crate::SkillManifest;

/// Parsed SKILL.md frontmatter.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Tool names this skill needs access to (e.g., ["file_read", "shell_exec"]).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Requirements (binaries, env vars, API keys).
    #[serde(default)]
    pub requires: Vec<SkillRequirementEntry>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// A requirement entry in the frontmatter.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillRequirementEntry {
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub check_command: Option<String>,
}

fn default_kind() -> String {
    "binary".to_string()
}

/// A loaded skill from a SKILL.md file.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    /// The parsed frontmatter.
    pub frontmatter: SkillFrontmatter,
    /// The markdown body (instructions for the fighter).
    pub body: String,
    /// Where this skill was loaded from.
    pub source_path: PathBuf,
    /// Precedence level (lower = higher priority).
    pub precedence: SkillPrecedence,
}

/// Skill loading precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillPrecedence {
    /// Workspace-local (highest priority)
    Workspace = 0,
    /// Installed from marketplace (~/.punch/skills/)
    Marketplace = 1,
    /// User global (~/.punch/skills/)
    User = 2,
    /// Bundled with Punch
    Bundled = 3,
}

impl std::fmt::Display for SkillPrecedence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace => write!(f, "workspace"),
            Self::Marketplace => write!(f, "marketplace"),
            Self::User => write!(f, "user"),
            Self::Bundled => write!(f, "bundled"),
        }
    }
}

/// Parse a SKILL.md file into frontmatter + body.
pub fn parse_skill_md(content: &str) -> PunchResult<(SkillFrontmatter, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err(punch_types::PunchError::Config(
            "SKILL.md must start with YAML frontmatter (---)".to_string(),
        ));
    }

    // Find the closing ---
    let rest = &content[3..];
    let end = rest.find("\n---").ok_or_else(|| {
        punch_types::PunchError::Config(
            "SKILL.md frontmatter not closed (missing closing ---)".to_string(),
        )
    })?;

    let yaml_str = rest[..end].trim();
    let body = rest[end + 4..].trim().to_string();

    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_str).map_err(|e| {
        punch_types::PunchError::Config(format!("invalid SKILL.md frontmatter: {}", e))
    })?;

    Ok((frontmatter, body))
}

/// Load a single skill from a directory.
///
/// Looks for SKILL.md in the given directory.
pub fn load_skill_from_dir(dir: &Path, precedence: SkillPrecedence) -> PunchResult<LoadedSkill> {
    let skill_path = dir.join("SKILL.md");
    if !skill_path.exists() {
        return Err(punch_types::PunchError::Config(format!(
            "no SKILL.md found in {}",
            dir.display()
        )));
    }

    let content = std::fs::read_to_string(&skill_path).map_err(|e| {
        punch_types::PunchError::Config(format!("failed to read {}: {}", skill_path.display(), e))
    })?;

    let (frontmatter, body) = parse_skill_md(&content)?;

    debug!(
        skill = %frontmatter.name,
        source = %precedence,
        path = %skill_path.display(),
        "loaded skill"
    );

    Ok(LoadedSkill {
        frontmatter,
        body,
        source_path: skill_path,
        precedence,
    })
}

/// Load all skills from a directory.
///
/// Each subdirectory that contains a SKILL.md is loaded as a skill.
pub fn load_skills_from_dir(dir: &Path, precedence: SkillPrecedence) -> Vec<LoadedSkill> {
    if !dir.exists() || !dir.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "failed to read skills directory");
            return vec![];
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            match load_skill_from_dir(&path, precedence) {
                Ok(skill) => skills.push(skill),
                Err(e) => {
                    debug!(path = %path.display(), error = %e, "skipping invalid skill directory");
                }
            }
        }
    }

    info!(
        count = skills.len(),
        source = %precedence,
        path = %dir.display(),
        "loaded skills from directory"
    );
    skills
}

/// Load all skills with precedence ordering.
///
/// Skills are loaded from (highest to lowest precedence):
/// 1. workspace_dir/skills/
/// 2. marketplace_dir/ (e.g. ~/.punch/skills/ for marketplace-installed)
/// 3. user_dir/ (user global skills)
/// 4. bundled_dir/skills/
///
/// When the same skill name appears at multiple levels, the highest
/// precedence version wins.
pub fn load_all_skills(
    workspace_dir: Option<&Path>,
    user_dir: Option<&Path>,
    bundled_dir: Option<&Path>,
) -> Vec<LoadedSkill> {
    load_all_skills_with_marketplace(workspace_dir, None, user_dir, bundled_dir)
}

/// Load all skills including marketplace-installed skills.
///
/// The `marketplace_dir` is typically `~/.punch/skills/` where marketplace
/// skills are installed to after verification.
pub fn load_all_skills_with_marketplace(
    workspace_dir: Option<&Path>,
    marketplace_dir: Option<&Path>,
    user_dir: Option<&Path>,
    bundled_dir: Option<&Path>,
) -> Vec<LoadedSkill> {
    let mut skills_by_name: HashMap<String, LoadedSkill> = HashMap::new();

    // Load in reverse precedence order (bundled first, workspace last overwrites)
    if let Some(dir) = bundled_dir {
        for skill in load_skills_from_dir(dir, SkillPrecedence::Bundled) {
            skills_by_name.insert(skill.frontmatter.name.clone(), skill);
        }
    }

    if let Some(dir) = user_dir {
        for skill in load_skills_from_dir(dir, SkillPrecedence::User) {
            skills_by_name.insert(skill.frontmatter.name.clone(), skill);
        }
    }

    if let Some(dir) = marketplace_dir {
        for skill in load_skills_from_dir(dir, SkillPrecedence::Marketplace) {
            skills_by_name.insert(skill.frontmatter.name.clone(), skill);
        }
    }

    if let Some(dir) = workspace_dir {
        for skill in load_skills_from_dir(dir, SkillPrecedence::Workspace) {
            skills_by_name.insert(skill.frontmatter.name.clone(), skill);
        }
    }

    let mut skills: Vec<LoadedSkill> = skills_by_name.into_values().collect();
    skills.sort_by(|a, b| a.frontmatter.name.cmp(&b.frontmatter.name));

    info!(total = skills.len(), "all skills loaded with precedence");
    skills
}

/// Convert a LoadedSkill into a SkillManifest.
impl From<&LoadedSkill> for SkillManifest {
    fn from(skill: &LoadedSkill) -> Self {
        let requirements = skill
            .frontmatter
            .requires
            .iter()
            .map(|r| crate::SkillRequirement {
                name: r.name.clone(),
                kind: match r.kind.as_str() {
                    "env_var" => crate::RequirementKind::EnvVar,
                    "api_key" => crate::RequirementKind::ApiKey,
                    _ => crate::RequirementKind::Binary,
                },
                check_command: r.check_command.clone(),
            })
            .collect();

        SkillManifest {
            name: skill.frontmatter.name.clone(),
            version: skill.frontmatter.version.clone(),
            description: skill.frontmatter.description.clone(),
            author: skill.frontmatter.author.clone(),
            tools: vec![], // Tools are referenced by name, not defined here
            requirements,
            skill_prompt: skill.body.clone(),
        }
    }
}

/// Render loaded skills as a system prompt section.
///
/// This produces the text that gets injected into the fighter's system
/// prompt so the LLM knows about its loaded skills.
pub fn render_skills_prompt(skills: &[LoadedSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("## Loaded Skills\n\n");
    out.push_str("You have the following skills loaded. Use them when relevant:\n\n");

    for skill in skills {
        out.push_str(&format!(
            "### {} (v{})\n",
            skill.frontmatter.name, skill.frontmatter.version
        ));
        if !skill.frontmatter.description.is_empty() {
            out.push_str(&format!("_{}_\n\n", skill.frontmatter.description));
        }
        if !skill.frontmatter.tools.is_empty() {
            out.push_str(&format!(
                "**Tools**: {}\n\n",
                skill.frontmatter.tools.join(", ")
            ));
        }
        out.push_str(&skill.body);
        out.push_str("\n\n---\n\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_skill_md_basic() {
        let content = r#"---
name: code-reviewer
version: 1.0.0
description: Expert code review
author: HumanCTO
tags: [code, review]
tools: [file_read, git_diff]
---

# Code Reviewer

When reviewing code, check for security issues first.
"#;
        let (fm, body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.name, "code-reviewer");
        assert_eq!(fm.version, "1.0.0");
        assert_eq!(fm.description, "Expert code review");
        assert_eq!(fm.tools, vec!["file_read", "git_diff"]);
        assert!(body.contains("Code Reviewer"));
        assert!(body.contains("security issues"));
    }

    #[test]
    fn test_parse_skill_md_minimal() {
        let content = r#"---
name: simple
---

Just a simple skill.
"#;
        let (fm, body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.name, "simple");
        assert_eq!(fm.version, "1.0.0"); // default
        assert!(body.contains("simple skill"));
    }

    #[test]
    fn test_parse_skill_md_with_requirements() {
        let content = r#"---
name: docker-ops
requires:
  - name: docker
    kind: binary
    check_command: docker --version
  - name: DOCKER_HOST
    kind: env_var
---

Docker operations skill.
"#;
        let (fm, _body) = parse_skill_md(content).unwrap();
        assert_eq!(fm.requires.len(), 2);
        assert_eq!(fm.requires[0].name, "docker");
        assert_eq!(fm.requires[0].kind, "binary");
        assert_eq!(fm.requires[1].kind, "env_var");
    }

    #[test]
    fn test_parse_skill_md_no_frontmatter() {
        let content = "# No frontmatter here";
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn test_parse_skill_md_unclosed_frontmatter() {
        let content = "---\nname: broken\n# No closing ---";
        assert!(parse_skill_md(content).is_err());
    }

    #[test]
    fn test_load_skill_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: A test\n---\n\nInstructions here.",
        )
        .unwrap();

        let skill = load_skill_from_dir(&skill_dir, SkillPrecedence::User).unwrap();
        assert_eq!(skill.frontmatter.name, "my-skill");
        assert_eq!(skill.precedence, SkillPrecedence::User);
        assert!(skill.body.contains("Instructions here"));
    }

    #[test]
    fn test_load_skills_from_dir() {
        let dir = tempfile::tempdir().unwrap();

        // Create two skill dirs
        for name in &["skill-a", "skill-b"] {
            let skill_dir = dir.path().join(name);
            fs::create_dir(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\n---\n\nBody for {name}."),
            )
            .unwrap();
        }

        // Also a non-skill dir (no SKILL.md)
        fs::create_dir(dir.path().join("not-a-skill")).unwrap();

        let skills = load_skills_from_dir(dir.path(), SkillPrecedence::Workspace);
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn test_load_all_skills_precedence() {
        let workspace = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();

        // Same skill name at both levels
        for (dir, name_suffix) in [(&workspace, "workspace"), (&user, "user")] {
            let skill_dir = dir.path().join("shared-skill");
            fs::create_dir(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!(
                    "---\nname: shared-skill\ndescription: from {name_suffix}\n---\n\nFrom {name_suffix}."
                ),
            )
            .unwrap();
        }

        let skills = load_all_skills(Some(workspace.path()), Some(user.path()), None);
        assert_eq!(skills.len(), 1);
        // Workspace should win
        assert_eq!(skills[0].frontmatter.description, "from workspace");
        assert_eq!(skills[0].precedence, SkillPrecedence::Workspace);
    }

    #[test]
    fn test_render_skills_prompt() {
        let content = "---\nname: test-skill\nversion: 2.0.0\ndescription: A test\ntools: [file_read]\n---\n\nDo the thing.";
        let (fm, body) = parse_skill_md(content).unwrap();
        let skill = LoadedSkill {
            frontmatter: fm,
            body,
            source_path: PathBuf::from("/tmp/test"),
            precedence: SkillPrecedence::Bundled,
        };

        let prompt = render_skills_prompt(&[skill]);
        assert!(prompt.contains("## Loaded Skills"));
        assert!(prompt.contains("test-skill (v2.0.0)"));
        assert!(prompt.contains("file_read"));
        assert!(prompt.contains("Do the thing"));
    }

    #[test]
    fn test_render_skills_prompt_empty() {
        let prompt = render_skills_prompt(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_loaded_skill_to_manifest() {
        let content = "---\nname: converter\nversion: 1.2.0\nauthor: test\nrequires:\n  - name: ffmpeg\n    kind: binary\n---\n\nConvert things.";
        let (fm, body) = parse_skill_md(content).unwrap();
        let skill = LoadedSkill {
            frontmatter: fm,
            body,
            source_path: PathBuf::from("/tmp"),
            precedence: SkillPrecedence::User,
        };
        let manifest: SkillManifest = SkillManifest::from(&skill);
        assert_eq!(manifest.name, "converter");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.requirements.len(), 1);
        assert_eq!(
            manifest.requirements[0].kind,
            crate::RequirementKind::Binary
        );
        assert!(manifest.skill_prompt.contains("Convert things"));
    }

    #[test]
    fn test_marketplace_precedence_display() {
        assert_eq!(SkillPrecedence::Marketplace.to_string(), "marketplace");
    }

    #[test]
    fn test_marketplace_precedence_ordering() {
        assert!(SkillPrecedence::Workspace < SkillPrecedence::Marketplace);
        assert!(SkillPrecedence::Marketplace < SkillPrecedence::User);
        assert!(SkillPrecedence::User < SkillPrecedence::Bundled);
    }

    #[test]
    fn test_load_all_skills_with_marketplace() {
        let workspace = tempfile::tempdir().unwrap();
        let marketplace = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();

        // Create a skill in marketplace dir
        let mp_skill = marketplace.path().join("mp-skill");
        fs::create_dir(&mp_skill).unwrap();
        fs::write(
            mp_skill.join("SKILL.md"),
            "---\nname: mp-skill\ndescription: from marketplace\n---\n\nMarketplace skill.",
        )
        .unwrap();

        // Create same skill in user dir (should be overridden by marketplace)
        let user_skill = user.path().join("mp-skill");
        fs::create_dir(&user_skill).unwrap();
        fs::write(
            user_skill.join("SKILL.md"),
            "---\nname: mp-skill\ndescription: from user\n---\n\nUser skill.",
        )
        .unwrap();

        let skills = load_all_skills_with_marketplace(
            Some(workspace.path()),
            Some(marketplace.path()),
            Some(user.path()),
            None,
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].frontmatter.description, "from marketplace");
        assert_eq!(skills[0].precedence, SkillPrecedence::Marketplace);
    }

    #[test]
    fn test_workspace_overrides_marketplace() {
        let workspace = tempfile::tempdir().unwrap();
        let marketplace = tempfile::tempdir().unwrap();

        for (dir, desc) in [(&workspace, "workspace"), (&marketplace, "marketplace")] {
            let skill_dir = dir.path().join("override-skill");
            fs::create_dir(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: override-skill\ndescription: from {desc}\n---\n\nBody."),
            )
            .unwrap();
        }

        let skills = load_all_skills_with_marketplace(
            Some(workspace.path()),
            Some(marketplace.path()),
            None,
            None,
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].frontmatter.description, "from workspace");
    }
}
