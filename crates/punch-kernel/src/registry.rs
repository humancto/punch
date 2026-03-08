//! Agent template registry.
//!
//! Loads [`FighterManifest`] templates from `agent.toml` files found in a
//! designated agents directory. Templates can then be looked up by name to
//! quickly spawn pre-configured fighters.

use std::collections::HashMap;
use std::path::Path;

use tracing::{debug, info, instrument, warn};

use punch_types::{FighterManifest, PunchError, PunchResult};

/// A registry of named agent templates.
///
/// Templates are loaded from TOML files and cached in memory. The registry is
/// **not** `Sync` by itself — wrap it in an `Arc<RwLock<_>>` if concurrent
/// access is needed (the [`Ring`](crate::ring::Ring) does this internally via
/// its own synchronization).
#[derive(Debug, Default)]
pub struct AgentRegistry {
    templates: HashMap<String, FighterManifest>,
}

impl AgentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `agents_dir` for agent templates and load them.
    ///
    /// The expected directory structure is:
    ///
    /// ```text
    /// agents/
    ///   coder/
    ///     agent.toml
    ///   reviewer/
    ///     agent.toml
    /// ```
    ///
    /// Each `agent.toml` is deserialized into a [`FighterManifest`]. The
    /// directory name is used as the template key (lowercased).
    #[instrument(skip(self), fields(dir = %agents_dir.display()))]
    pub fn load_templates(&mut self, agents_dir: &Path) -> PunchResult<()> {
        if !agents_dir.is_dir() {
            return Err(PunchError::Config(format!(
                "agents directory does not exist: {}",
                agents_dir.display()
            )));
        }

        let entries = std::fs::read_dir(agents_dir).map_err(|e| {
            PunchError::Config(format!(
                "failed to read agents directory {}: {}",
                agents_dir.display(),
                e
            ))
        })?;

        let mut loaded = 0usize;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "failed to read directory entry");
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let toml_path = path.join("agent.toml");
            if !toml_path.exists() {
                debug!(dir = %path.display(), "skipping directory without agent.toml");
                continue;
            }

            let template_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            if template_name.is_empty() {
                warn!(dir = %path.display(), "could not determine template name");
                continue;
            }

            match self.load_single_template(&toml_path) {
                Ok(manifest) => {
                    info!(template = %template_name, name = %manifest.name, "loaded agent template");
                    self.templates.insert(template_name, manifest);
                    loaded += 1;
                }
                Err(e) => {
                    warn!(
                        template = %template_name,
                        error = %e,
                        "failed to load agent template"
                    );
                }
            }
        }

        info!(loaded, "agent template scan complete");
        Ok(())
    }

    /// Load a single `agent.toml` file into a [`FighterManifest`].
    fn load_single_template(&self, path: &Path) -> PunchResult<FighterManifest> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PunchError::Config(format!("failed to read {}: {}", path.display(), e)))?;

        let manifest: FighterManifest = toml::from_str(&content).map_err(|e| {
            PunchError::Config(format!("failed to parse {}: {}", path.display(), e))
        })?;

        Ok(manifest)
    }

    /// Retrieve a template by name (case-insensitive lookup).
    pub fn get_template(&self, name: &str) -> Option<&FighterManifest> {
        self.templates.get(&name.to_lowercase())
    }

    /// List all registered template names.
    pub fn list_templates(&self) -> Vec<String> {
        let mut names: Vec<String> = self.templates.keys().cloned().collect();
        names.sort();
        names
    }

    /// Register a template manually (e.g. from an API call).
    pub fn register(&mut self, name: String, manifest: FighterManifest) {
        self.templates.insert(name.to_lowercase(), manifest);
    }

    /// Remove a template by name.
    pub fn unregister(&mut self, name: &str) -> Option<FighterManifest> {
        self.templates.remove(&name.to_lowercase())
    }

    /// Number of registered templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}
