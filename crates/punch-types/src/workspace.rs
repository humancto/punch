//! # Workspace Context — mapping the battlefield before the fight begins.
//!
//! This module tracks the working context of an agent's environment, including
//! project structure, open files, recent changes, and git status, providing
//! situational awareness for tactical decisions.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The type of project detected in the workspace — identifying the fighting discipline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    /// Rust project (Cargo.toml).
    Rust,
    /// Python project (pyproject.toml, setup.py, requirements.txt).
    Python,
    /// JavaScript project (package.json without tsconfig).
    JavaScript,
    /// TypeScript project (tsconfig.json).
    TypeScript,
    /// Go project (go.mod).
    Go,
    /// Java project (pom.xml, build.gradle).
    Java,
    /// Unknown or unrecognized project type.
    Unknown,
}

/// The type of change made to a file — classifying the maneuver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    /// A new file was created.
    Created,
    /// An existing file was modified.
    Modified,
    /// A file was deleted.
    Deleted,
    /// A file was renamed (contains the old name).
    Renamed(String),
}

/// A currently active/open file — a weapon in the fighter's hands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveFile {
    /// Path to the file.
    pub path: PathBuf,
    /// Programming language of the file.
    pub language: String,
    /// When the file was last modified.
    pub last_modified: DateTime<Utc>,
    /// Number of lines in the file.
    pub line_count: usize,
}

/// A recorded file change — a move logged in the fight record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path to the changed file.
    pub path: PathBuf,
    /// Type of change.
    pub change_type: ChangeType,
    /// When the change occurred.
    pub timestamp: DateTime<Utc>,
}

/// Git repository information — the battle formation's version control intel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitInfo {
    /// Current branch name.
    pub branch: String,
    /// Current commit hash.
    pub commit: String,
    /// Whether there are uncommitted changes.
    pub is_dirty: bool,
    /// Remote URL if configured.
    pub remote_url: Option<String>,
}

/// The full workspace context — complete situational awareness for the fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceContext {
    /// Root path of the workspace/project.
    pub root_path: PathBuf,
    /// Detected project type.
    pub project_type: Option<ProjectType>,
    /// Currently active/open files.
    pub active_files: Vec<ActiveFile>,
    /// Recent file changes.
    pub recent_changes: Vec<FileChange>,
    /// Git repository information.
    pub git_info: Option<GitInfo>,
}

impl WorkspaceContext {
    /// Create a new workspace context for the given root path — enter the arena.
    pub fn new(root_path: PathBuf) -> Self {
        let project_type = Self::detect_project_type(&root_path);
        Self {
            root_path,
            project_type,
            active_files: Vec::new(),
            recent_changes: Vec::new(),
            git_info: None,
        }
    }

    /// Detect the project type from marker files in the root directory — identify the fighting style.
    pub fn detect_project_type(root: &Path) -> Option<ProjectType> {
        if root.join("Cargo.toml").exists() {
            Some(ProjectType::Rust)
        } else if root.join("go.mod").exists() {
            Some(ProjectType::Go)
        } else if root.join("tsconfig.json").exists() {
            Some(ProjectType::TypeScript)
        } else if root.join("package.json").exists() {
            Some(ProjectType::JavaScript)
        } else if root.join("pyproject.toml").exists()
            || root.join("setup.py").exists()
            || root.join("requirements.txt").exists()
        {
            Some(ProjectType::Python)
        } else if root.join("pom.xml").exists() || root.join("build.gradle").exists() {
            Some(ProjectType::Java)
        } else {
            None
        }
    }

    /// Add an active file to the context — equip a new weapon.
    pub fn add_active_file(&mut self, file: ActiveFile) {
        self.active_files.push(file);
    }

    /// Record a file change — log a combat move.
    pub fn record_change(&mut self, change: FileChange) {
        self.recent_changes.push(change);
    }

    /// Get the most recently active files — review the fighter's current loadout.
    pub fn recent_files(&self, limit: usize) -> Vec<&ActiveFile> {
        let mut files: Vec<&ActiveFile> = self.active_files.iter().collect();
        files.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
        files.truncate(limit);
        files
    }

    /// Generate a text summary of the workspace — the battlefield briefing for the system prompt.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!("Workspace: {}", self.root_path.display()));

        if let Some(ref pt) = self.project_type {
            parts.push(format!("Project type: {pt:?}"));
        }

        if !self.active_files.is_empty() {
            parts.push(format!("Active files: {}", self.active_files.len()));
            for file in self.recent_files(5) {
                parts.push(format!(
                    "  - {} ({}, {} lines)",
                    file.path.display(),
                    file.language,
                    file.line_count
                ));
            }
        }

        if !self.recent_changes.is_empty() {
            parts.push(format!("Recent changes: {}", self.recent_changes.len()));
        }

        if let Some(ref git) = self.git_info {
            parts.push(format!("Git branch: {}", git.branch));
            parts.push(format!("Git commit: {}", &git.commit));
            if git.is_dirty {
                parts.push("Git status: dirty (uncommitted changes)".to_string());
            }
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir_with_file(filename: &str) -> TempDir {
        let dir = TempDir::new().expect("create temp dir");
        fs::write(dir.path().join(filename), "").expect("create marker file");
        dir
    }

    #[test]
    fn test_detect_project_type_rust() {
        let dir = temp_dir_with_file("Cargo.toml");
        let detected = WorkspaceContext::detect_project_type(dir.path());
        assert_eq!(detected, Some(ProjectType::Rust));
    }

    #[test]
    fn test_detect_project_type_javascript() {
        let dir = temp_dir_with_file("package.json");
        let detected = WorkspaceContext::detect_project_type(dir.path());
        assert_eq!(detected, Some(ProjectType::JavaScript));
    }

    #[test]
    fn test_active_files() {
        let mut ctx = WorkspaceContext {
            root_path: PathBuf::from("/tmp/project"),
            project_type: Some(ProjectType::Rust),
            active_files: Vec::new(),
            recent_changes: Vec::new(),
            git_info: None,
        };

        let file1 = ActiveFile {
            path: PathBuf::from("src/main.rs"),
            language: "rust".to_string(),
            last_modified: Utc::now(),
            line_count: 100,
        };

        let file2 = ActiveFile {
            path: PathBuf::from("src/lib.rs"),
            language: "rust".to_string(),
            last_modified: Utc::now(),
            line_count: 250,
        };

        ctx.add_active_file(file1);
        ctx.add_active_file(file2);

        assert_eq!(ctx.active_files.len(), 2);

        let recent = ctx.recent_files(1);
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn test_recent_changes() {
        let mut ctx = WorkspaceContext::new(PathBuf::from("/tmp/project"));

        ctx.record_change(FileChange {
            path: PathBuf::from("src/main.rs"),
            change_type: ChangeType::Modified,
            timestamp: Utc::now(),
        });

        ctx.record_change(FileChange {
            path: PathBuf::from("src/new_module.rs"),
            change_type: ChangeType::Created,
            timestamp: Utc::now(),
        });

        ctx.record_change(FileChange {
            path: PathBuf::from("src/old.rs"),
            change_type: ChangeType::Renamed("src/new.rs".to_string()),
            timestamp: Utc::now(),
        });

        assert_eq!(ctx.recent_changes.len(), 3);
        assert_eq!(ctx.recent_changes[0].change_type, ChangeType::Modified);
        assert_eq!(ctx.recent_changes[1].change_type, ChangeType::Created);
    }

    #[test]
    fn test_summary_generation() {
        let mut ctx = WorkspaceContext {
            root_path: PathBuf::from("/home/fighter/project"),
            project_type: Some(ProjectType::Rust),
            active_files: vec![ActiveFile {
                path: PathBuf::from("src/main.rs"),
                language: "rust".to_string(),
                last_modified: Utc::now(),
                line_count: 42,
            }],
            recent_changes: Vec::new(),
            git_info: Some(GitInfo {
                branch: "main".to_string(),
                commit: "abc1234".to_string(),
                is_dirty: true,
                remote_url: Some("https://github.com/humancto/punch".to_string()),
            }),
        };

        ctx.record_change(FileChange {
            path: PathBuf::from("src/main.rs"),
            change_type: ChangeType::Modified,
            timestamp: Utc::now(),
        });

        let summary = ctx.summary();

        assert!(summary.contains("/home/fighter/project"));
        assert!(summary.contains("Rust"));
        assert!(summary.contains("Active files: 1"));
        assert!(summary.contains("src/main.rs"));
        assert!(summary.contains("42 lines"));
        assert!(summary.contains("Git branch: main"));
        assert!(summary.contains("dirty"));
    }

    #[test]
    fn test_git_info() {
        let git = GitInfo {
            branch: "feat/new-move".to_string(),
            commit: "deadbeef1234567890".to_string(),
            is_dirty: false,
            remote_url: Some("git@github.com:humancto/punch.git".to_string()),
        };

        let json = serde_json::to_string(&git).expect("serialize git info");
        let deser: GitInfo = serde_json::from_str(&json).expect("deserialize git info");

        assert_eq!(deser.branch, "feat/new-move");
        assert_eq!(deser.commit, "deadbeef1234567890");
        assert!(!deser.is_dirty);
        assert!(deser.remote_url.is_some());
    }
}
