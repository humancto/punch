//! Subprocess sandbox — the containment ring for agent-spawned processes.
//!
//! Provides environment sanitization, path traversal prevention, command
//! validation, and a restricted execution environment for shell commands
//! run by agents. Every subprocess enters the sandboxed arena, where only
//! approved paths, environment variables, and commands are permitted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Configuration for the subprocess containment ring.
///
/// Defines what paths, environment variables, and commands are permitted
/// within the sandboxed arena. Deny rules always take precedence over
/// allow rules — a fighter cannot punch through a denied path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Directories the subprocess is allowed to access.
    pub allowed_paths: Vec<PathBuf>,
    /// Directories explicitly barred from the arena (e.g., /etc/shadow, ~/.ssh).
    pub denied_paths: Vec<PathBuf>,
    /// Environment variable names to pass through the containment ring.
    pub env_allowlist: Vec<String>,
    /// Environment variable patterns to block (supports glob: `*_TOKEN`, `AWS_*`).
    pub env_denylist: Vec<String>,
    /// Maximum bytes of stdout+stderr to capture from the subprocess.
    pub max_output_bytes: usize,
    /// Maximum execution time in seconds before the subprocess is killed.
    pub max_execution_secs: u64,
    /// Whether to allow network access from the subprocess.
    pub allow_network: bool,
    /// Explicit working directory (must reside within allowed_paths).
    pub working_dir: Option<PathBuf>,
    /// Command prefixes that are unconditionally denied.
    pub denied_commands: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/root"));

        Self {
            allowed_paths: vec![
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                PathBuf::from("/tmp"),
            ],
            denied_paths: vec![
                PathBuf::from("/etc/shadow"),
                PathBuf::from("/etc/passwd"),
                home.join(".ssh"),
                home.join(".gnupg"),
                home.join(".aws"),
            ],
            env_allowlist: vec![
                "PATH".into(),
                "HOME".into(),
                "USER".into(),
                "LANG".into(),
                "LC_ALL".into(),
                "TERM".into(),
                "SHELL".into(),
                "TMPDIR".into(),
            ],
            env_denylist: vec![
                "*_SECRET*".into(),
                "*_TOKEN".into(),
                "*_PASSWORD".into(),
                "*_KEY".into(),
                "AWS_*".into(),
                "GITHUB_TOKEN".into(),
            ],
            max_output_bytes: 1_048_576, // 1 MB
            max_execution_secs: 120,
            allow_network: true,
            working_dir: None,
            denied_commands: vec![
                "rm -rf /".into(),
                "rm -rf /*".into(),
                "dd if=/dev".into(),
                "mkfs".into(),
                ":(){ :|:& };:".into(),
                "chmod -R 777 /".into(),
                "> /dev/sda".into(),
            ],
        }
    }
}

/// Violations detected by the containment ring.
///
/// Each variant describes how a fighter attempted to escape the sandboxed
/// arena or violate an enforced constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxViolation {
    /// A denied command was detected.
    DeniedCommand { command: String, reason: String },
    /// A path traversal attempt was detected.
    PathTraversal {
        path: String,
        attempted_escape: String,
    },
    /// Access to a denied path was attempted.
    DeniedPath { path: String },
    /// Access to a path outside the allowed set was attempted.
    PathNotAllowed { path: String },
    /// A denied environment variable was referenced.
    DeniedEnvironment { var_name: String },
}

impl std::fmt::Display for SandboxViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxViolation::DeniedCommand { command, reason } => {
                write!(
                    f,
                    "sandbox violation: denied command '{}' — {}",
                    command, reason
                )
            }
            SandboxViolation::PathTraversal {
                path,
                attempted_escape,
            } => {
                write!(
                    f,
                    "sandbox violation: path traversal in '{}' — attempted escape via '{}'",
                    path, attempted_escape
                )
            }
            SandboxViolation::DeniedPath { path } => {
                write!(f, "sandbox violation: access to denied path '{}'", path)
            }
            SandboxViolation::PathNotAllowed { path } => {
                write!(
                    f,
                    "sandbox violation: path '{}' is outside allowed directories",
                    path
                )
            }
            SandboxViolation::DeniedEnvironment { var_name } => {
                write!(
                    f,
                    "sandbox violation: environment variable '{}' is denied",
                    var_name
                )
            }
        }
    }
}

impl std::error::Error for SandboxViolation {}

/// The enforcer that guards the sandboxed arena.
///
/// Validates commands, paths, and environment variables before any subprocess
/// is allowed to enter the containment ring.
#[derive(Debug, Clone)]
pub struct SandboxEnforcer {
    /// The containment ring configuration.
    pub config: SandboxConfig,
}

impl SandboxEnforcer {
    /// Create a new enforcer with the given containment ring configuration.
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Create a new enforcer with default containment ring settings.
    pub fn with_defaults() -> Self {
        Self::new(SandboxConfig::default())
    }

    /// Pre-execution validation: check a command before it enters the arena.
    ///
    /// Detects denied command prefixes, path traversal attempts, and
    /// shell injection patterns (backticks, `$()`, pipes to sensitive commands).
    pub fn validate_command(&self, command: &str) -> Result<(), SandboxViolation> {
        let trimmed = command.trim();

        // Check against denied command prefixes.
        for denied in &self.config.denied_commands {
            if trimmed.starts_with(denied.as_str()) || trimmed.contains(denied.as_str()) {
                return Err(SandboxViolation::DeniedCommand {
                    command: trimmed.to_string(),
                    reason: format!("matches denied pattern '{}'", denied),
                });
            }
        }

        // Detect shell injection via backticks.
        if trimmed.contains('`') {
            // Allow backticks only if they appear inside single quotes, which is
            // hard to determine reliably. For safety, flag all backticks.
            return Err(SandboxViolation::DeniedCommand {
                command: trimmed.to_string(),
                reason: "backtick shell injection detected".into(),
            });
        }

        // Detect $() command substitution.
        if trimmed.contains("$(") {
            return Err(SandboxViolation::DeniedCommand {
                command: trimmed.to_string(),
                reason: "$() command substitution detected".into(),
            });
        }

        // Detect pipes to sensitive commands.
        let sensitive_pipe_targets = ["sh", "bash", "eval", "exec", "sudo"];
        if trimmed.contains('|') {
            for segment in trimmed.split('|').skip(1) {
                let target = segment.split_whitespace().next().unwrap_or("");
                for sensitive in &sensitive_pipe_targets {
                    if target == *sensitive {
                        return Err(SandboxViolation::DeniedCommand {
                            command: trimmed.to_string(),
                            reason: format!("pipe to sensitive command '{}' detected", sensitive),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate whether a path is accessible within the containment ring.
    ///
    /// Canonicalizes the path, then checks denied paths first (deny always wins),
    /// followed by allowed paths. A fighter cannot reach outside its arena.
    pub fn validate_path(&self, path: &Path) -> Result<(), SandboxViolation> {
        // Canonicalize the path. If the path doesn't exist yet, we fall back
        // to a manual resolution approach to catch traversal attempts.
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // Path doesn't exist — resolve manually by normalizing components.
                self.normalize_path(path)
            }
        };

        let canonical_str = canonical.display().to_string();

        // Deny list takes precedence — no fighter punches through a denied path.
        for denied in &self.config.denied_paths {
            let denied_canonical = match denied.canonicalize() {
                Ok(p) => p,
                Err(_) => self.normalize_path(denied),
            };
            if canonical.starts_with(&denied_canonical) {
                return Err(SandboxViolation::DeniedPath {
                    path: canonical_str,
                });
            }
        }

        // Check if the path falls within any allowed directory.
        if self.config.allowed_paths.is_empty() {
            return Err(SandboxViolation::PathNotAllowed {
                path: canonical_str,
            });
        }

        let mut inside_allowed = false;
        for allowed in &self.config.allowed_paths {
            let allowed_canonical = match allowed.canonicalize() {
                Ok(p) => p,
                Err(_) => self.normalize_path(allowed),
            };
            if canonical.starts_with(&allowed_canonical) {
                inside_allowed = true;
                break;
            }
        }

        if !inside_allowed {
            // Check if this is a traversal attempt.
            let path_str = path.display().to_string();
            if path_str.contains("..") {
                return Err(SandboxViolation::PathTraversal {
                    path: path_str,
                    attempted_escape: canonical_str,
                });
            }
            return Err(SandboxViolation::PathNotAllowed {
                path: canonical_str,
            });
        }

        Ok(())
    }

    /// Build a clean environment — only variables that survive the containment ring.
    ///
    /// Starts with an empty environment, then includes only variables from the
    /// allowlist that exist in the current process environment. Any variable
    /// matching a denylist pattern is filtered out, even if it appears on the
    /// allowlist.
    pub fn sanitize_environment(&self) -> Vec<(String, String)> {
        let current_env: Vec<(String, String)> = std::env::vars().collect();
        let mut sanitized = Vec::new();

        for (key, value) in &current_env {
            // Check if the variable is on the allowlist.
            if !self.config.env_allowlist.contains(key) {
                continue;
            }

            // Check if the variable matches any denylist pattern.
            if self.matches_env_denylist(key) {
                continue;
            }

            sanitized.push((key.clone(), value.clone()));
        }

        sanitized
    }

    /// Build a sandboxed `tokio::process::Command` ready to enter the arena.
    ///
    /// Validates the command, sets a sanitized environment via `env_clear()` +
    /// individual `env()` calls, and configures the working directory.
    pub fn build_command(
        &self,
        command: &str,
    ) -> Result<tokio::process::Command, SandboxViolation> {
        // Validate before the fighter enters the ring.
        self.validate_command(command)?;

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);

        // Sanitize the environment — strip everything, then add approved vars.
        cmd.env_clear();
        for (key, value) in self.sanitize_environment() {
            cmd.env(&key, &value);
        }

        // Set the working directory.
        if let Some(ref wd) = self.config.working_dir {
            cmd.current_dir(wd);
        }

        Ok(cmd)
    }

    /// Check if an environment variable name matches any denylist pattern.
    fn matches_env_denylist(&self, var_name: &str) -> bool {
        for pattern in &self.config.env_denylist {
            if glob_match(pattern, var_name) {
                // Special exception: PATH should never be denied by *_KEY pattern.
                if var_name == "PATH" && pattern.contains("_KEY") {
                    continue;
                }
                return true;
            }
        }
        false
    }

    /// Normalize a path by resolving `.` and `..` components and following
    /// symlinks on the closest existing ancestor. Used when the target
    /// doesn't exist yet but we still need canonical path resolution
    /// (e.g., `/tmp` -> `/private/tmp` on macOS).
    fn normalize_path(&self, path: &Path) -> PathBuf {
        // Start with the current directory if the path is relative.
        let effective = if path.is_relative() {
            if let Some(ref wd) = self.config.working_dir {
                wd.join(path)
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("/"))
                    .join(path)
            }
        } else {
            path.to_path_buf()
        };

        // First, do a logical normalization (resolve . and ..).
        let mut logical_components = Vec::new();
        for component in effective.components() {
            match component {
                std::path::Component::ParentDir => {
                    logical_components.pop();
                }
                std::path::Component::CurDir => {}
                other => {
                    logical_components.push(other.as_os_str().to_os_string());
                }
            }
        }

        let mut logical = PathBuf::new();
        for c in &logical_components {
            logical.push(c);
        }
        if logical.as_os_str().is_empty() {
            logical = PathBuf::from("/");
        }

        // Try to canonicalize the closest existing ancestor, then append
        // the remaining non-existent suffix. This resolves symlinks like
        // /tmp -> /private/tmp on macOS.
        let mut ancestor = logical.clone();
        let mut suffix_parts = Vec::new();
        loop {
            if ancestor.exists() {
                if let Ok(real) = ancestor.canonicalize() {
                    let mut result = real;
                    for part in suffix_parts.into_iter().rev() {
                        result.push(part);
                    }
                    return result;
                }
                break;
            }
            if let Some(file_name) = ancestor.file_name() {
                suffix_parts.push(file_name.to_os_string());
                if !ancestor.pop() {
                    break;
                }
            } else {
                break;
            }
        }

        logical
    }
}

/// Simple glob-style pattern matching for environment variable names.
///
/// Supports `*` as a wildcard that matches any sequence of characters.
/// For example, `*_TOKEN` matches `GITHUB_TOKEN`, and `AWS_*` matches `AWS_SECRET_ACCESS_KEY`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        // No wildcard — exact match.
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(found) => {
                // First part must match at the start if the pattern doesn't begin with *.
                if i == 0 && found != 0 {
                    return false;
                }
                pos += found + part.len();
            }
            None => return false,
        }
    }

    // If the pattern doesn't end with *, the text must end at `pos`.
    if !pattern.ends_with('*') {
        return pos == text.len();
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: Default config has sensible values
    // -----------------------------------------------------------------------
    #[test]
    fn test_default_config_sensible_values() {
        let config = SandboxConfig::default();

        assert!(!config.allowed_paths.is_empty());
        assert!(config.allowed_paths.contains(&PathBuf::from("/tmp")));
        assert!(!config.denied_paths.is_empty());
        assert!(!config.env_allowlist.is_empty());
        assert!(config.env_allowlist.contains(&"PATH".to_string()));
        assert!(config.env_allowlist.contains(&"HOME".to_string()));
        assert!(!config.env_denylist.is_empty());
        assert_eq!(config.max_output_bytes, 1_048_576);
        assert_eq!(config.max_execution_secs, 120);
        assert!(config.allow_network);
        assert!(config.working_dir.is_none());
        assert!(!config.denied_commands.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: validate_command allows normal commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_command_allows_normal_commands() {
        let enforcer = SandboxEnforcer::with_defaults();

        assert!(enforcer.validate_command("ls -la").is_ok());
        assert!(enforcer.validate_command("cat README.md").is_ok());
        assert!(enforcer.validate_command("grep -r 'pattern' src/").is_ok());
        assert!(enforcer.validate_command("cargo build").is_ok());
        assert!(enforcer.validate_command("echo hello").is_ok());
    }

    // -----------------------------------------------------------------------
    // Test 3: validate_command blocks denied commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_command_blocks_denied_commands() {
        let enforcer = SandboxEnforcer::with_defaults();

        let result = enforcer.validate_command("rm -rf /");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::DeniedCommand { command, reason } => {
                assert!(command.contains("rm -rf /"));
                assert!(reason.contains("denied pattern"));
            }
            other => panic!("expected DeniedCommand, got {:?}", other),
        }

        assert!(enforcer.validate_command("rm -rf /*").is_err());
        assert!(
            enforcer
                .validate_command("dd if=/dev/zero of=disk.img")
                .is_err()
        );
        assert!(enforcer.validate_command("mkfs.ext4 /dev/sda1").is_err());
    }

    // -----------------------------------------------------------------------
    // Test 4: validate_command detects fork bomb
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_command_detects_fork_bomb() {
        let enforcer = SandboxEnforcer::with_defaults();

        let result = enforcer.validate_command(":(){ :|:& };:");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::DeniedCommand { reason, .. } => {
                assert!(reason.contains("denied pattern"));
            }
            other => panic!("expected DeniedCommand, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 5: validate_path allows files in allowed directories
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_path_allows_files_in_allowed_dirs() {
        let mut config = SandboxConfig::default();
        config.allowed_paths = vec![PathBuf::from("/tmp")];
        config.denied_paths = vec![];
        let enforcer = SandboxEnforcer::new(config);

        assert!(enforcer.validate_path(Path::new("/tmp/test.txt")).is_ok());
        assert!(
            enforcer
                .validate_path(Path::new("/tmp/subdir/file.rs"))
                .is_ok()
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: validate_path blocks files in denied directories
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_path_blocks_denied_dirs() {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/root"));

        let mut config = SandboxConfig::default();
        config.allowed_paths = vec![home.clone()];
        let enforcer = SandboxEnforcer::new(config);

        let ssh_key = home.join(".ssh/id_rsa");
        let result = enforcer.validate_path(&ssh_key);
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::DeniedPath { path } => {
                assert!(path.contains(".ssh"));
            }
            other => panic!("expected DeniedPath, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 7: validate_path detects path traversal
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_path_detects_traversal() {
        let mut config = SandboxConfig::default();
        config.allowed_paths = vec![PathBuf::from("/tmp/sandbox")];
        config.denied_paths = vec![];
        let enforcer = SandboxEnforcer::new(config);

        // Attempting to escape /tmp/sandbox via ../../etc/passwd
        let result = enforcer.validate_path(Path::new("/tmp/sandbox/../../etc/passwd"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::PathTraversal {
                path,
                attempted_escape,
            } => {
                assert!(path.contains(".."));
                assert!(attempted_escape.contains("etc"));
            }
            SandboxViolation::PathNotAllowed { .. } => {
                // Also acceptable — the path is outside allowed dirs.
            }
            other => panic!("expected PathTraversal or PathNotAllowed, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 8: validate_path handles symlink-style traversal
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_path_handles_symlink_traversal() {
        let mut config = SandboxConfig::default();
        config.allowed_paths = vec![PathBuf::from("/tmp/arena")];
        config.denied_paths = vec![];
        let enforcer = SandboxEnforcer::new(config);

        // A path that looks like it's in /tmp/arena but escapes via ..
        let result = enforcer.validate_path(Path::new("/tmp/arena/../../../etc/shadow"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Test 9: sanitize_environment only passes allowed vars
    // -----------------------------------------------------------------------
    #[test]
    fn test_sanitize_environment_only_allowed_vars() {
        let enforcer = SandboxEnforcer::with_defaults();
        let env = enforcer.sanitize_environment();

        // All returned vars must be in the allowlist.
        for (key, _) in &env {
            assert!(
                enforcer.config.env_allowlist.contains(key),
                "unexpected env var '{}' passed through sanitization",
                key
            );
        }

        // PATH should be present if it exists in the system environment.
        if std::env::var("PATH").is_ok() {
            assert!(
                env.iter().any(|(k, _)| k == "PATH"),
                "PATH should be in sanitized environment"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 10: sanitize_environment filters denied patterns
    // -----------------------------------------------------------------------
    #[test]
    fn test_sanitize_environment_filters_denied_patterns() {
        let mut config = SandboxConfig::default();
        // Add a secret-looking var to the allowlist to test that denylist wins.
        config.env_allowlist.push("MY_SECRET_KEY".to_string());
        config.env_allowlist.push("AWS_ACCESS_KEY_ID".to_string());
        let enforcer = SandboxEnforcer::new(config);

        // Set the env vars for this test.
        // SAFETY: This test is not run in parallel with other tests that read these vars.
        unsafe {
            std::env::set_var("MY_SECRET_KEY", "should-be-denied");
            std::env::set_var("AWS_ACCESS_KEY_ID", "should-be-denied");
        }

        let env = enforcer.sanitize_environment();

        // These should be filtered out by denylist patterns.
        assert!(
            !env.iter().any(|(k, _)| k == "MY_SECRET_KEY"),
            "MY_SECRET_KEY should be filtered by *_SECRET* pattern"
        );
        assert!(
            !env.iter().any(|(k, _)| k == "AWS_ACCESS_KEY_ID"),
            "AWS_ACCESS_KEY_ID should be filtered by AWS_* pattern"
        );

        // Clean up.
        // SAFETY: This test is not run in parallel with other tests that read these vars.
        unsafe {
            std::env::remove_var("MY_SECRET_KEY");
            std::env::remove_var("AWS_ACCESS_KEY_ID");
        }
    }

    // -----------------------------------------------------------------------
    // Test 11: build_command creates command with sanitized env
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_command_creates_sanitized_command() {
        let enforcer = SandboxEnforcer::with_defaults();
        let result = enforcer.build_command("ls -la");
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Test 12: build_command fails for denied commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_command_fails_for_denied_commands() {
        let enforcer = SandboxEnforcer::with_defaults();
        let result = enforcer.build_command("rm -rf /");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::DeniedCommand { .. } => {}
            other => panic!("expected DeniedCommand, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 13: Custom config overrides defaults
    // -----------------------------------------------------------------------
    #[test]
    fn test_custom_config_overrides_defaults() {
        let config = SandboxConfig {
            allowed_paths: vec![PathBuf::from("/opt/arena")],
            denied_paths: vec![PathBuf::from("/opt/arena/secrets")],
            env_allowlist: vec!["CUSTOM_VAR".into()],
            env_denylist: vec![],
            max_output_bytes: 512,
            max_execution_secs: 30,
            allow_network: false,
            working_dir: Some(PathBuf::from("/opt/arena")),
            denied_commands: vec!["danger".into()],
        };

        let enforcer = SandboxEnforcer::new(config.clone());
        assert_eq!(enforcer.config.max_output_bytes, 512);
        assert_eq!(enforcer.config.max_execution_secs, 30);
        assert!(!enforcer.config.allow_network);
        assert_eq!(enforcer.config.allowed_paths.len(), 1);
        assert_eq!(enforcer.config.denied_commands, vec!["danger".to_string()]);

        // Custom denied command should be blocked.
        assert!(enforcer.validate_command("danger zone").is_err());
        // Default denied commands should not be blocked (custom config replaced them).
        assert!(enforcer.validate_command("rm -rf /").is_ok());
    }

    // -----------------------------------------------------------------------
    // Test 14: Empty allowed_paths denies all paths
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_allowed_paths_denies_all() {
        let config = SandboxConfig {
            allowed_paths: vec![],
            denied_paths: vec![],
            ..SandboxConfig::default()
        };
        let enforcer = SandboxEnforcer::new(config);

        let result = enforcer.validate_path(Path::new("/tmp/anything"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxViolation::PathNotAllowed { .. } => {}
            other => panic!("expected PathNotAllowed, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 15: DeniedCommand display formatting
    // -----------------------------------------------------------------------
    #[test]
    fn test_denied_command_display_formatting() {
        let violation = SandboxViolation::DeniedCommand {
            command: "rm -rf /".into(),
            reason: "matches denied pattern".into(),
        };
        let display = format!("{}", violation);
        assert!(display.contains("sandbox violation"));
        assert!(display.contains("rm -rf /"));
        assert!(display.contains("matches denied pattern"));

        let traversal = SandboxViolation::PathTraversal {
            path: "../../etc/passwd".into(),
            attempted_escape: "/etc/passwd".into(),
        };
        let display = format!("{}", traversal);
        assert!(display.contains("path traversal"));
        assert!(display.contains("../../etc/passwd"));

        let denied_path = SandboxViolation::DeniedPath {
            path: "/etc/shadow".into(),
        };
        let display = format!("{}", denied_path);
        assert!(display.contains("denied path"));

        let not_allowed = SandboxViolation::PathNotAllowed {
            path: "/root/secret".into(),
        };
        let display = format!("{}", not_allowed);
        assert!(display.contains("outside allowed"));

        let denied_env = SandboxViolation::DeniedEnvironment {
            var_name: "AWS_SECRET_KEY".into(),
        };
        let display = format!("{}", denied_env);
        assert!(display.contains("denied"));
        assert!(display.contains("AWS_SECRET_KEY"));
    }

    // -----------------------------------------------------------------------
    // Test 16: Path canonicalization handles relative paths
    // -----------------------------------------------------------------------
    #[test]
    fn test_path_canonicalization_relative() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut config = SandboxConfig::default();
        config.allowed_paths = vec![cwd.clone()];
        config.denied_paths = vec![];
        config.working_dir = Some(cwd.clone());
        let enforcer = SandboxEnforcer::new(config);

        // A relative path like "src/main.rs" should be resolved against cwd.
        let normalized = enforcer.normalize_path(Path::new("src/main.rs"));
        assert!(normalized.is_absolute());
        assert!(normalized.starts_with(&cwd));
    }

    // -----------------------------------------------------------------------
    // Additional tests: glob matching
    // -----------------------------------------------------------------------
    #[test]
    fn test_glob_match_patterns() {
        assert!(glob_match("*_TOKEN", "GITHUB_TOKEN"));
        assert!(glob_match("*_TOKEN", "SLACK_TOKEN"));
        assert!(!glob_match("*_TOKEN", "GITHUB_TOKEN_EXTRA"));
        assert!(glob_match("AWS_*", "AWS_SECRET_ACCESS_KEY"));
        assert!(glob_match("AWS_*", "AWS_REGION"));
        assert!(!glob_match("AWS_*", "NOT_AWS"));
        assert!(glob_match("*_SECRET*", "MY_SECRET_KEY"));
        assert!(glob_match("*_SECRET*", "DB_SECRET"));
        assert!(glob_match("EXACT", "EXACT"));
        assert!(!glob_match("EXACT", "NOT_EXACT"));
    }

    // -----------------------------------------------------------------------
    // Test: validate_command detects command substitution
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_command_detects_substitution() {
        let enforcer = SandboxEnforcer::with_defaults();

        assert!(enforcer.validate_command("echo $(whoami)").is_err());
        assert!(enforcer.validate_command("echo `whoami`").is_err());
    }

    // -----------------------------------------------------------------------
    // Test: validate_command detects pipe to sensitive commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_validate_command_detects_pipe_to_sensitive() {
        let enforcer = SandboxEnforcer::with_defaults();

        assert!(enforcer.validate_command("cat file | sh").is_err());
        assert!(enforcer.validate_command("echo cmd | bash").is_err());
        assert!(enforcer.validate_command("echo cmd | sudo rm").is_err());

        // Pipe to non-sensitive commands should be fine.
        assert!(enforcer.validate_command("ls | grep pattern").is_ok());
        assert!(enforcer.validate_command("cat file | wc -l").is_ok());
    }
}
