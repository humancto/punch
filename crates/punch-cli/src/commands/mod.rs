//! CLI command implementations.

pub mod config;
pub mod fighter;
pub mod gorilla;
pub mod init;
pub mod moves;
pub mod start;
pub mod status;

use std::path::PathBuf;

use punch_types::PunchConfig;

/// Resolve the Punch home directory (~/.punch/).
pub fn punch_home() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".punch")
}

/// Resolve the config file path, using a CLI override or the default.
pub fn config_path(override_path: Option<&str>) -> PathBuf {
    match override_path {
        Some(p) => PathBuf::from(p),
        None => punch_home().join("config.toml"),
    }
}

/// Load the Punch config from disk.
pub fn load_config(override_path: Option<&str>) -> Result<PunchConfig, String> {
    let path = config_path(override_path);
    if !path.exists() {
        return Err(format!(
            "Config file not found at {}. Run `punch init` first.",
            path.display()
        ));
    }
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {}", e))?;
    toml::from_str(&contents).map_err(|e| format!("Failed to parse config: {}", e))
}

/// Load the .env file from ~/.punch/.env if it exists.
pub fn load_dotenv() {
    let env_path = punch_home().join(".env");
    if env_path.exists()
        && let Ok(contents) = std::fs::read_to_string(&env_path)
    {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');
                if std::env::var(key).is_err() {
                    // SAFETY: We only call this during single-threaded CLI
                    // startup, before spawning any async tasks.
                    unsafe { std::env::set_var(key, value) };
                }
            }
        }
    }
}
