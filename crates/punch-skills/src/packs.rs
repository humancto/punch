//! # Skill Packs
//!
//! Bundled MCP server configurations that install with one command.
//!
//! A skill pack is a directory containing a `SKILLPACK.toml` file that describes
//! one or more MCP servers, a skill prompt, and environment variable requirements.
//! Installing a pack adds the MCP server configs to `~/.punch/config.toml` and
//! writes the skill prompt to `~/.punch/skills/<pack-name>/SKILL.md`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use punch_types::error::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// TOML deserialization types (mirrors SKILLPACK.toml structure)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SkillPackToml {
    pack: PackMeta,
    mcp_servers: Vec<PackMcpServerToml>,
    skill: SkillSection,
    env_vars: Option<EnvVarsSection>,
}

#[derive(Debug, Deserialize)]
struct PackMeta {
    name: String,
    version: String,
    description: String,
    author: String,
    category: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PackMcpServerToml {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    install_command: Option<String>,
    setup_command: Option<String>,
    description: String,
}

#[derive(Debug, Deserialize)]
struct SkillSection {
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct EnvVarsSection {
    #[serde(default)]
    required: Vec<String>,
    #[serde(default)]
    optional: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A parsed skill pack ready for installation.
#[derive(Debug, Clone)]
pub struct SkillPack {
    /// Unique name of the pack.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Author or team.
    pub author: String,
    /// Category (e.g. "productivity", "developer").
    pub category: String,
    /// Searchable tags.
    pub tags: Vec<String>,
    /// MCP servers this pack provides.
    pub mcp_servers: Vec<PackMcpServer>,
    /// Skill prompt injected into the system prompt.
    pub skill_prompt: String,
    /// Environment variables that must be set.
    pub required_env_vars: Vec<String>,
    /// Environment variables that may optionally be set.
    pub optional_env_vars: Vec<String>,
}

/// An MCP server configuration within a skill pack.
#[derive(Debug, Clone)]
pub struct PackMcpServer {
    /// Server name (used as the key in config.toml).
    pub name: String,
    /// Command to run the server.
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
    /// Command to install the server (e.g. `pip install ...`).
    pub install_command: Option<String>,
    /// Command to run after install for initial setup (e.g. auth flow).
    pub setup_command: Option<String>,
    /// Human-readable description.
    pub description: String,
}

/// Result of installing a skill pack.
#[derive(Debug)]
pub struct InstallResult {
    /// The pack that was installed.
    pub pack_name: String,
    /// MCP servers added to config.
    pub servers_added: Vec<String>,
    /// Path to the skill prompt file.
    pub skill_path: PathBuf,
    /// Required env vars that are not yet set.
    pub missing_env_vars: Vec<String>,
}

// ---------------------------------------------------------------------------
// Bundled packs
// ---------------------------------------------------------------------------

/// Directory containing bundled SKILLPACK.toml files, relative to this file.
const BUNDLED_PACKS: &[(&str, &str)] = &[
    (
        "productivity",
        include_str!("../bundled-packs/productivity/SKILLPACK.toml"),
    ),
    (
        "developer",
        include_str!("../bundled-packs/developer/SKILLPACK.toml"),
    ),
    (
        "research",
        include_str!("../bundled-packs/research/SKILLPACK.toml"),
    ),
    (
        "files",
        include_str!("../bundled-packs/files/SKILLPACK.toml"),
    ),
];

/// Load all bundled skill packs that ship with Punch.
pub fn load_bundled_packs() -> Vec<SkillPack> {
    let mut packs = Vec::new();
    for (name, content) in BUNDLED_PACKS {
        match parse_skillpack_toml(content) {
            Ok(pack) => {
                info!(pack = %pack.name, "loaded bundled skill pack");
                packs.push(pack);
            }
            Err(e) => {
                warn!(pack = %name, error = %e, "failed to load bundled skill pack");
            }
        }
    }
    packs
}

/// Load a skill pack from a SKILLPACK.toml file on disk.
pub fn load_pack_from_path(path: &Path) -> PunchResult<SkillPack> {
    let toml_path = if path.is_dir() {
        path.join("SKILLPACK.toml")
    } else {
        path.to_path_buf()
    };

    let content = fs::read_to_string(&toml_path).map_err(|e| {
        PunchError::Config(format!(
            "failed to read SKILLPACK.toml at {}: {}",
            toml_path.display(),
            e
        ))
    })?;

    parse_skillpack_toml(&content)
}

/// List available bundled packs as `(name, description)` pairs.
pub fn available_packs() -> Vec<(String, String)> {
    load_bundled_packs()
        .into_iter()
        .map(|p| (p.name, p.description))
        .collect()
}

/// Find a bundled pack by name.
pub fn find_bundled_pack(name: &str) -> Option<SkillPack> {
    let name_lower = name.to_lowercase();
    load_bundled_packs()
        .into_iter()
        .find(|p| p.name.to_lowercase() == name_lower)
}

// ---------------------------------------------------------------------------
// Pack installer
// ---------------------------------------------------------------------------

/// Install a skill pack into the Punch configuration.
///
/// This function:
/// 1. Appends MCP server configs to `~/.punch/config.toml`
/// 2. Writes the skill prompt to `~/.punch/skills/<pack-name>/SKILL.md`
/// 3. Returns which env vars still need to be set
pub fn install_pack(punch_home: &Path, pack: &SkillPack) -> PunchResult<InstallResult> {
    info!(pack = %pack.name, "installing skill pack");

    // 1. Append MCP server configs to config.toml
    let config_path = punch_home.join("config.toml");
    let mut servers_added = Vec::new();

    for server in &pack.mcp_servers {
        append_mcp_server_config(&config_path, &pack.name, server)?;
        servers_added.push(server.name.clone());
    }

    // 2. Write skill prompt to ~/.punch/skills/<pack-name>/SKILL.md
    let skill_dir = punch_home.join("skills").join(&pack.name);
    fs::create_dir_all(&skill_dir).map_err(|e| {
        PunchError::Config(format!(
            "failed to create skill directory {}: {}",
            skill_dir.display(),
            e
        ))
    })?;

    let skill_path = skill_dir.join("SKILL.md");
    let skill_content = format!(
        "---\n\
         name: {}\n\
         version: {}\n\
         description: {}\n\
         author: {}\n\
         category: {}\n\
         tags: [{}]\n\
         ---\n\n\
         {}\n",
        pack.name,
        pack.version,
        pack.description,
        pack.author,
        pack.category,
        pack.tags.join(", "),
        pack.skill_prompt,
    );

    fs::write(&skill_path, &skill_content).map_err(|e| {
        PunchError::Config(format!(
            "failed to write skill file {}: {}",
            skill_path.display(),
            e
        ))
    })?;

    // 3. Check which required env vars are missing
    let missing_env_vars: Vec<String> = pack
        .required_env_vars
        .iter()
        .filter(|var| std::env::var(var).is_err())
        .cloned()
        .collect();

    info!(
        pack = %pack.name,
        servers = ?servers_added,
        missing_vars = ?missing_env_vars,
        "skill pack installed"
    );

    Ok(InstallResult {
        pack_name: pack.name.clone(),
        servers_added,
        skill_path,
        missing_env_vars,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a SKILLPACK.toml string into a `SkillPack`.
fn parse_skillpack_toml(content: &str) -> PunchResult<SkillPack> {
    let parsed: SkillPackToml = toml::from_str(content)
        .map_err(|e| PunchError::Config(format!("invalid SKILLPACK.toml: {}", e)))?;

    let env_vars = parsed.env_vars.unwrap_or(EnvVarsSection {
        required: Vec::new(),
        optional: Vec::new(),
    });

    Ok(SkillPack {
        name: parsed.pack.name,
        version: parsed.pack.version,
        description: parsed.pack.description,
        author: parsed.pack.author,
        category: parsed.pack.category,
        tags: parsed.pack.tags,
        mcp_servers: parsed
            .mcp_servers
            .into_iter()
            .map(|s| PackMcpServer {
                name: s.name,
                command: s.command,
                args: s.args,
                install_command: s.install_command,
                setup_command: s.setup_command,
                description: s.description,
            })
            .collect(),
        skill_prompt: parsed.skill.prompt,
        required_env_vars: env_vars.required,
        optional_env_vars: env_vars.optional,
    })
}

/// Append an MCP server configuration block to config.toml.
///
/// Uses string append (like channel.rs) to preserve existing comments and formatting.
fn append_mcp_server_config(
    config_path: &Path,
    pack_name: &str,
    server: &PackMcpServer,
) -> PunchResult<()> {
    use std::fs::OpenOptions;

    // Build the TOML block
    let args_toml = if server.args.is_empty() {
        "[]".to_string()
    } else {
        let quoted: Vec<String> = server.args.iter().map(|a| format!("\"{}\"", a)).collect();
        format!("[{}]", quoted.join(", "))
    };

    let toml_block = format!(
        "\n# Skill pack: {pack_name}\n\
         [mcp_servers.{name}]\n\
         command = \"{command}\"\n\
         args = {args}\n",
        pack_name = pack_name,
        name = server.name,
        command = server.command,
        args = args_toml,
    );

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_path)
        .map_err(|e| PunchError::Config(format!("failed to open config: {}", e)))?;

    file.write_all(toml_block.as_bytes())
        .map_err(|e| PunchError::Config(format!("failed to write config: {}", e)))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_bundled_packs() {
        let packs = load_bundled_packs();
        assert!(
            packs.len() >= 4,
            "expected at least 4 bundled packs, got {}",
            packs.len()
        );

        let names: Vec<&str> = packs.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"productivity"));
        assert!(names.contains(&"developer"));
        assert!(names.contains(&"research"));
        assert!(names.contains(&"files"));
    }

    #[test]
    fn test_bundled_packs_have_mcp_servers() {
        let packs = load_bundled_packs();
        for pack in &packs {
            assert!(
                !pack.mcp_servers.is_empty(),
                "pack '{}' should have at least one MCP server",
                pack.name
            );
        }
    }

    #[test]
    fn test_bundled_packs_have_prompts() {
        let packs = load_bundled_packs();
        for pack in &packs {
            assert!(
                !pack.skill_prompt.is_empty(),
                "pack '{}' should have a non-empty skill prompt",
                pack.name
            );
        }
    }

    #[test]
    fn test_bundled_packs_have_descriptions() {
        let packs = load_bundled_packs();
        for pack in &packs {
            assert!(
                !pack.description.is_empty(),
                "pack '{}' should have a non-empty description",
                pack.name
            );
        }
    }

    #[test]
    fn test_available_packs() {
        let packs = available_packs();
        assert!(packs.len() >= 4);
        let names: Vec<&str> = packs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"productivity"));
        assert!(names.contains(&"developer"));
    }

    #[test]
    fn test_find_bundled_pack() {
        let pack = find_bundled_pack("productivity");
        assert!(pack.is_some());
        let pack = pack.unwrap();
        assert_eq!(pack.name, "productivity");
        assert_eq!(pack.author, "humancto");
    }

    #[test]
    fn test_find_bundled_pack_case_insensitive() {
        let pack = find_bundled_pack("DEVELOPER");
        assert!(pack.is_some());
        assert_eq!(pack.unwrap().name, "developer");
    }

    #[test]
    fn test_find_bundled_pack_not_found() {
        let pack = find_bundled_pack("nonexistent");
        assert!(pack.is_none());
    }

    #[test]
    fn test_parse_productivity_pack() {
        let content = include_str!("../bundled-packs/productivity/SKILLPACK.toml");
        let pack = parse_skillpack_toml(content).unwrap();
        assert_eq!(pack.name, "productivity");
        assert_eq!(pack.version, "1.0.0");
        assert_eq!(pack.mcp_servers.len(), 1);
        assert_eq!(pack.mcp_servers[0].name, "localmind");
        assert_eq!(pack.mcp_servers[0].command, "python3");
        assert!(pack.mcp_servers[0].install_command.is_some());
        assert!(pack.mcp_servers[0].setup_command.is_some());
        assert!(pack.required_env_vars.is_empty());
        assert!(!pack.optional_env_vars.is_empty());
    }

    #[test]
    fn test_parse_developer_pack() {
        let content = include_str!("../bundled-packs/developer/SKILLPACK.toml");
        let pack = parse_skillpack_toml(content).unwrap();
        assert_eq!(pack.name, "developer");
        assert_eq!(pack.mcp_servers[0].name, "github");
        assert!(!pack.required_env_vars.is_empty());
        assert!(pack
            .required_env_vars
            .contains(&"GITHUB_PERSONAL_ACCESS_TOKEN".to_string()));
    }

    #[test]
    fn test_parse_invalid_toml() {
        let result = parse_skillpack_toml("this is not valid toml {{{}}}");
        assert!(result.is_err());
    }

    #[test]
    fn test_install_pack_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let punch_home = tmp.path();

        // Create a minimal config.toml
        let config_path = punch_home.join("config.toml");
        fs::write(&config_path, "# Punch config\n").unwrap();

        let pack = SkillPack {
            name: "test-pack".to_string(),
            version: "1.0.0".to_string(),
            description: "A test pack".to_string(),
            author: "tester".to_string(),
            category: "test".to_string(),
            tags: vec!["test".to_string()],
            mcp_servers: vec![PackMcpServer {
                name: "test-server".to_string(),
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                install_command: None,
                setup_command: None,
                description: "A test server".to_string(),
            }],
            skill_prompt: "You are a test skill.".to_string(),
            required_env_vars: vec![],
            optional_env_vars: vec![],
        };

        let result = install_pack(punch_home, &pack).unwrap();

        // Verify config was updated
        let config_content = fs::read_to_string(&config_path).unwrap();
        assert!(config_content.contains("[mcp_servers.test-server]"));
        assert!(config_content.contains("command = \"echo\""));
        assert!(config_content.contains("# Skill pack: test-pack"));

        // Verify skill file was created
        assert!(result.skill_path.exists());
        let skill_content = fs::read_to_string(&result.skill_path).unwrap();
        assert!(skill_content.contains("You are a test skill."));
        assert!(skill_content.contains("name: test-pack"));

        // Verify result
        assert_eq!(result.pack_name, "test-pack");
        assert_eq!(result.servers_added, vec!["test-server"]);
        assert!(result.missing_env_vars.is_empty());
    }

    #[test]
    fn test_install_pack_reports_missing_env_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let punch_home = tmp.path();
        fs::write(punch_home.join("config.toml"), "").unwrap();

        let pack = SkillPack {
            name: "env-test".to_string(),
            version: "1.0.0".to_string(),
            description: "Test".to_string(),
            author: "tester".to_string(),
            category: "test".to_string(),
            tags: vec![],
            mcp_servers: vec![],
            skill_prompt: "test".to_string(),
            required_env_vars: vec!["PUNCH_TEST_VERY_UNLIKELY_VAR_12345".to_string()],
            optional_env_vars: vec![],
        };

        let result = install_pack(punch_home, &pack).unwrap();
        assert_eq!(
            result.missing_env_vars,
            vec!["PUNCH_TEST_VERY_UNLIKELY_VAR_12345"]
        );
    }

    #[test]
    fn test_install_pack_multiple_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let punch_home = tmp.path();
        fs::write(punch_home.join("config.toml"), "").unwrap();

        let pack = SkillPack {
            name: "multi".to_string(),
            version: "1.0.0".to_string(),
            description: "Multi-server pack".to_string(),
            author: "tester".to_string(),
            category: "test".to_string(),
            tags: vec![],
            mcp_servers: vec![
                PackMcpServer {
                    name: "server-a".to_string(),
                    command: "node".to_string(),
                    args: vec!["a.js".to_string()],
                    install_command: None,
                    setup_command: None,
                    description: "Server A".to_string(),
                },
                PackMcpServer {
                    name: "server-b".to_string(),
                    command: "python3".to_string(),
                    args: vec!["-m".to_string(), "b".to_string()],
                    install_command: None,
                    setup_command: None,
                    description: "Server B".to_string(),
                },
            ],
            skill_prompt: "test".to_string(),
            required_env_vars: vec![],
            optional_env_vars: vec![],
        };

        let result = install_pack(punch_home, &pack).unwrap();
        assert_eq!(result.servers_added.len(), 2);

        let config = fs::read_to_string(punch_home.join("config.toml")).unwrap();
        assert!(config.contains("[mcp_servers.server-a]"));
        assert!(config.contains("[mcp_servers.server-b]"));
    }

    #[test]
    fn test_load_pack_from_path_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pack_dir = tmp.path().join("mypack");
        fs::create_dir_all(&pack_dir).unwrap();

        let toml_content = r#"
[pack]
name = "mypack"
version = "0.1.0"
description = "A custom pack"
author = "tester"
category = "custom"
tags = ["custom"]

[[mcp_servers]]
name = "my-server"
command = "node"
args = ["server.js"]
description = "My custom server"

[skill]
prompt = "You have custom tools."

[env_vars]
required = []
optional = []
"#;
        fs::write(pack_dir.join("SKILLPACK.toml"), toml_content).unwrap();

        let pack = load_pack_from_path(&pack_dir).unwrap();
        assert_eq!(pack.name, "mypack");
        assert_eq!(pack.mcp_servers.len(), 1);
    }

    #[test]
    fn test_load_pack_from_path_file() {
        let tmp = tempfile::tempdir().unwrap();
        let toml_path = tmp.path().join("SKILLPACK.toml");

        let toml_content = r#"
[pack]
name = "direct"
version = "0.1.0"
description = "Direct file load"
author = "tester"
category = "test"

[[mcp_servers]]
name = "srv"
command = "echo"
args = []
description = "Echo server"

[skill]
prompt = "Test."
"#;
        fs::write(&toml_path, toml_content).unwrap();

        let pack = load_pack_from_path(&toml_path).unwrap();
        assert_eq!(pack.name, "direct");
    }

    #[test]
    fn test_load_pack_from_nonexistent_path() {
        let result = load_pack_from_path(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_mcp_server_args_formatting() {
        let tmp = tempfile::tempdir().unwrap();
        let punch_home = tmp.path();
        fs::write(punch_home.join("config.toml"), "").unwrap();

        let pack = SkillPack {
            name: "args-test".to_string(),
            version: "1.0.0".to_string(),
            description: "Test".to_string(),
            author: "tester".to_string(),
            category: "test".to_string(),
            tags: vec![],
            mcp_servers: vec![PackMcpServer {
                name: "npx-server".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/server-fetch".to_string()],
                install_command: None,
                setup_command: None,
                description: "Test".to_string(),
            }],
            skill_prompt: "test".to_string(),
            required_env_vars: vec![],
            optional_env_vars: vec![],
        };

        install_pack(punch_home, &pack).unwrap();

        let config = fs::read_to_string(punch_home.join("config.toml")).unwrap();
        assert!(config.contains(r#"args = ["-y", "@mcp/server-fetch"]"#));
    }

    #[test]
    fn test_empty_args_formatting() {
        let tmp = tempfile::tempdir().unwrap();
        let punch_home = tmp.path();
        fs::write(punch_home.join("config.toml"), "").unwrap();

        let pack = SkillPack {
            name: "no-args".to_string(),
            version: "1.0.0".to_string(),
            description: "Test".to_string(),
            author: "tester".to_string(),
            category: "test".to_string(),
            tags: vec![],
            mcp_servers: vec![PackMcpServer {
                name: "simple".to_string(),
                command: "server".to_string(),
                args: vec![],
                install_command: None,
                setup_command: None,
                description: "Test".to_string(),
            }],
            skill_prompt: "test".to_string(),
            required_env_vars: vec![],
            optional_env_vars: vec![],
        };

        install_pack(punch_home, &pack).unwrap();

        let config = fs::read_to_string(punch_home.join("config.toml")).unwrap();
        assert!(config.contains("args = []"));
    }
}
