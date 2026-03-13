//! # WASM Plugin Sandbox
//!
//! A trait-based plugin system for importing special moves from external sources.
//! Plugins are like imported techniques — foreign fighting styles that a fighter
//! can learn and execute within a sandboxed arena.
//!
//! The runtime abstraction allows backing by wasmtime, wasmer, or even native
//! plugin implementations, keeping the core system free of any specific WASM
//! engine dependency.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use punch_types::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Plugin Manifest — the scroll describing an imported technique
// ---------------------------------------------------------------------------

/// Describes a plugin's identity, entry point, and sandbox constraints.
///
/// Every imported technique must declare what it needs before stepping into
/// the ring. The manifest is the contract between the plugin and the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Human-readable name for the plugin (the technique's name).
    pub name: String,
    /// Semantic version string (e.g. "1.0.0").
    pub version: String,
    /// Brief description of what this plugin does.
    pub description: String,
    /// Author or organization that created the plugin.
    pub author: String,
    /// The entry-point function name to invoke (the opening move).
    pub entry_point: String,
    /// Capabilities this plugin requires to operate.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Maximum memory the plugin may consume, in bytes (default: 64 MB).
    #[serde(default = "default_max_memory")]
    pub max_memory_bytes: u64,
    /// Maximum wall-clock execution time in milliseconds (default: 30 000).
    #[serde(default = "default_max_execution_ms")]
    pub max_execution_ms: u64,
    /// Fine-grained permissions controlling what the plugin may access.
    #[serde(default)]
    pub permissions: PluginPermissions,
}

fn default_max_memory() -> u64 {
    64 * 1024 * 1024 // 64 MB
}

fn default_max_execution_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// Plugin Permissions — the rules of engagement
// ---------------------------------------------------------------------------

/// Controls what system resources a plugin is allowed to touch.
///
/// By default, all permissions are denied — a plugin starts in full lockdown
/// and must explicitly request access to any external resource.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPermissions {
    /// Whether the plugin may open network connections.
    #[serde(default)]
    pub allow_network: bool,
    /// Filesystem path patterns the plugin may access.
    #[serde(default)]
    pub allow_filesystem: Vec<String>,
    /// Environment variable names the plugin may read.
    #[serde(default)]
    pub allow_env_vars: Vec<String>,
    /// Whether the plugin may spawn subprocesses.
    #[serde(default)]
    pub allow_subprocess: bool,
}

// ---------------------------------------------------------------------------
// Plugin State — where the technique is in its lifecycle
// ---------------------------------------------------------------------------

/// Lifecycle state of a loaded plugin instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginState {
    /// Plugin bytecode is loaded and ready to fight.
    Loaded,
    /// Plugin is currently executing a move.
    Running,
    /// Plugin has been stopped (cornered).
    Stopped,
    /// Plugin encountered a fatal error and is knocked out.
    Error(String),
}

// ---------------------------------------------------------------------------
// Plugin Instance — a living, breathing imported technique
// ---------------------------------------------------------------------------

/// A loaded plugin with its manifest, state, and runtime statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstance {
    /// Unique identifier for this plugin instance.
    pub id: Uuid,
    /// The manifest that describes this plugin's contract.
    pub manifest: PluginManifest,
    /// Current lifecycle state.
    pub state: PluginState,
    /// When this plugin was loaded into the ring.
    pub loaded_at: DateTime<Utc>,
    /// When the plugin last executed a move.
    pub last_invoked: Option<DateTime<Utc>>,
    /// Total number of times this plugin has been invoked.
    pub invocation_count: u64,
    /// Cumulative execution time across all invocations (ms).
    pub total_execution_ms: u64,
}

// ---------------------------------------------------------------------------
// Plugin I/O — the strike and the counter
// ---------------------------------------------------------------------------

/// Input payload sent to a plugin when invoking a function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInput {
    /// Name of the function to call within the plugin.
    pub function: String,
    /// Arguments passed to the function.
    pub args: serde_json::Value,
    /// Execution context (metadata, caller info, etc.).
    pub context: serde_json::Value,
}

/// Output returned by a plugin after execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOutput {
    /// The result value produced by the plugin.
    pub result: serde_json::Value,
    /// Log lines emitted during execution.
    pub logs: Vec<String>,
    /// Wall-clock execution time in milliseconds.
    pub execution_ms: u64,
    /// Peak memory usage in bytes during execution.
    pub memory_used_bytes: u64,
}

// ---------------------------------------------------------------------------
// Plugin Runtime Trait — the dojo where techniques are practised
// ---------------------------------------------------------------------------

/// Abstraction over the underlying execution engine (wasmtime, wasmer, native).
///
/// Each dojo teaches imported techniques in its own way, but all dojos
/// respect the same interface so the ring doesn't care which one is in use.
#[async_trait]
pub trait PluginRuntime: Send + Sync {
    /// Load a plugin from its manifest and raw bytecode.
    async fn load(&self, manifest: &PluginManifest, wasm_bytes: &[u8]) -> PunchResult<Uuid>;

    /// Invoke a function on a loaded plugin.
    async fn invoke(&self, plugin_id: &Uuid, input: PluginInput) -> PunchResult<PluginOutput>;

    /// Unload a plugin, freeing its resources.
    async fn unload(&self, plugin_id: &Uuid) -> PunchResult<()>;

    /// List all currently loaded plugin instances.
    fn list_plugins(&self) -> Vec<PluginInstance>;
}

// ---------------------------------------------------------------------------
// Plugin Registry — the master scroll of imported techniques
// ---------------------------------------------------------------------------

/// Central registry that tracks all loaded plugins and delegates execution
/// to the configured runtime.
///
/// The registry is the gatekeeper — it validates manifests, tracks plugin
/// lifecycles, and ensures every imported technique plays by the rules.
pub struct PluginRegistry {
    /// All registered plugin instances, keyed by their unique ID.
    plugins: DashMap<Uuid, PluginInstance>,
    /// The optional runtime that actually executes plugin code.
    runtime: Option<Arc<dyn PluginRuntime>>,
}

impl PluginRegistry {
    /// Create a new registry with no runtime (registration-only mode).
    pub fn new() -> Self {
        Self {
            plugins: DashMap::new(),
            runtime: None,
        }
    }

    /// Create a new registry backed by a specific runtime dojo.
    pub fn with_runtime(runtime: Arc<dyn PluginRuntime>) -> Self {
        Self {
            plugins: DashMap::new(),
            runtime: Some(runtime),
        }
    }

    /// Register a new plugin from its manifest and bytecode.
    ///
    /// If a runtime is configured, the plugin is loaded into it.
    /// Otherwise the plugin is registered in metadata-only mode.
    pub async fn register(&self, manifest: PluginManifest, bytes: &[u8]) -> PunchResult<Uuid> {
        let errors = Self::validate_manifest(&manifest);
        if !errors.is_empty() {
            return Err(PunchError::Config(format!(
                "invalid plugin manifest: {}",
                errors.join("; ")
            )));
        }

        let id = if let Some(ref rt) = self.runtime {
            rt.load(&manifest, bytes).await?
        } else {
            Uuid::new_v4()
        };

        let instance = PluginInstance {
            id,
            manifest: manifest.clone(),
            state: PluginState::Loaded,
            loaded_at: Utc::now(),
            last_invoked: None,
            invocation_count: 0,
            total_execution_ms: 0,
        };

        info!(
            plugin_id = %id,
            name = %manifest.name,
            version = %manifest.version,
            "imported new technique into the registry"
        );

        self.plugins.insert(id, instance);
        Ok(id)
    }

    /// Invoke a function on a registered plugin.
    ///
    /// Requires a runtime to be configured. Updates invocation statistics
    /// on the plugin instance after execution.
    pub async fn invoke(&self, plugin_id: &Uuid, input: PluginInput) -> PunchResult<PluginOutput> {
        let rt = self.runtime.as_ref().ok_or_else(|| {
            PunchError::Internal("no plugin runtime configured — cannot execute technique".into())
        })?;

        // Verify plugin exists
        if !self.plugins.contains_key(plugin_id) {
            return Err(PunchError::Internal(format!(
                "plugin {plugin_id} not found in the registry"
            )));
        }

        // Mark as running
        if let Some(mut entry) = self.plugins.get_mut(plugin_id) {
            entry.state = PluginState::Running;
        }

        debug!(plugin_id = %plugin_id, function = %input.function, "executing imported technique");

        let result = rt.invoke(plugin_id, input).await;

        // Update stats based on outcome
        if let Some(mut entry) = self.plugins.get_mut(plugin_id) {
            match &result {
                Ok(output) => {
                    entry.state = PluginState::Loaded;
                    entry.invocation_count += 1;
                    entry.total_execution_ms += output.execution_ms;
                    entry.last_invoked = Some(Utc::now());
                }
                Err(e) => {
                    entry.state = PluginState::Error(e.to_string());
                    warn!(plugin_id = %plugin_id, error = %e, "technique execution failed");
                }
            }
        }

        result
    }

    /// Unregister a plugin, removing it from the registry and runtime.
    pub async fn unregister(&self, plugin_id: &Uuid) -> PunchResult<()> {
        if self.plugins.remove(plugin_id).is_none() {
            return Err(PunchError::Internal(format!(
                "plugin {plugin_id} not found in the registry"
            )));
        }

        if let Some(ref rt) = self.runtime {
            rt.unload(plugin_id).await?;
        }

        info!(plugin_id = %plugin_id, "removed imported technique from registry");
        Ok(())
    }

    /// List all registered plugin instances.
    pub fn list(&self) -> Vec<PluginInstance> {
        self.plugins.iter().map(|e| e.value().clone()).collect()
    }

    /// Look up a plugin by its unique ID.
    pub fn get(&self, plugin_id: &Uuid) -> Option<PluginInstance> {
        self.plugins.get(plugin_id).map(|e| e.value().clone())
    }

    /// Find a plugin by name (returns the first match).
    pub fn get_by_name(&self, name: &str) -> Option<PluginInstance> {
        self.plugins
            .iter()
            .find(|e| e.value().manifest.name == name)
            .map(|e| e.value().clone())
    }

    /// Return the total number of registered plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Validate a plugin manifest and return a list of validation errors.
    ///
    /// An empty list means the manifest is valid and the technique may enter
    /// the ring.
    pub fn validate_manifest(manifest: &PluginManifest) -> Vec<String> {
        let mut errors = Vec::new();

        if manifest.name.trim().is_empty() {
            errors.push("plugin name must not be empty".to_string());
        }

        if manifest.version.trim().is_empty() {
            errors.push("plugin version must not be empty".to_string());
        }

        if manifest.entry_point.trim().is_empty() {
            errors.push("plugin entry_point must not be empty".to_string());
        }

        if manifest.max_memory_bytes == 0 {
            errors.push("max_memory_bytes must be greater than zero".to_string());
        }

        if manifest.max_execution_ms == 0 {
            errors.push("max_execution_ms must be greater than zero".to_string());
        }

        errors
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Native Plugin Runtime — a dojo for home-grown techniques
// ---------------------------------------------------------------------------

type NativePluginFn = Box<dyn Fn(PluginInput) -> PunchResult<PluginOutput> + Send + Sync>;

/// A simple native-code plugin runtime for testing and plugins that don't
/// need WASM isolation.
///
/// Instead of compiling bytecode, this dojo registers plain Rust closures
/// as special moves — useful for built-in techniques and unit tests.
pub struct NativePluginRuntime {
    /// Registered plugin functions keyed by plugin ID.
    functions: DashMap<Uuid, NativePluginFn>,
    /// Plugin instances for bookkeeping.
    instances: DashMap<Uuid, PluginInstance>,
}

impl NativePluginRuntime {
    /// Create a new native plugin runtime.
    pub fn new() -> Self {
        Self {
            functions: DashMap::new(),
            instances: DashMap::new(),
        }
    }

    /// Register a native function as a plugin implementation.
    ///
    /// Call this after `load()` to associate a callable with the plugin ID.
    pub fn register_function<F>(&self, plugin_id: Uuid, f: F)
    where
        F: Fn(PluginInput) -> PunchResult<PluginOutput> + Send + Sync + 'static,
    {
        self.functions.insert(plugin_id, Box::new(f));
    }
}

impl Default for NativePluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginRuntime for NativePluginRuntime {
    async fn load(&self, manifest: &PluginManifest, _wasm_bytes: &[u8]) -> PunchResult<Uuid> {
        let id = Uuid::new_v4();
        let instance = PluginInstance {
            id,
            manifest: manifest.clone(),
            state: PluginState::Loaded,
            loaded_at: Utc::now(),
            last_invoked: None,
            invocation_count: 0,
            total_execution_ms: 0,
        };
        self.instances.insert(id, instance);
        info!(plugin_id = %id, name = %manifest.name, "loaded native technique");
        Ok(id)
    }

    async fn invoke(&self, plugin_id: &Uuid, input: PluginInput) -> PunchResult<PluginOutput> {
        let func = self.functions.get(plugin_id).ok_or_else(|| {
            PunchError::Internal(format!(
                "no native function registered for plugin {plugin_id}"
            ))
        })?;

        let start = std::time::Instant::now();
        let mut output = func(input)?;
        output.execution_ms = start.elapsed().as_millis() as u64;

        // Update internal bookkeeping
        if let Some(mut inst) = self.instances.get_mut(plugin_id) {
            inst.invocation_count += 1;
            inst.total_execution_ms += output.execution_ms;
            inst.last_invoked = Some(Utc::now());
        }

        Ok(output)
    }

    async fn unload(&self, plugin_id: &Uuid) -> PunchResult<()> {
        self.functions.remove(plugin_id);
        self.instances.remove(plugin_id);
        info!(plugin_id = %plugin_id, "unloaded native technique");
        Ok(())
    }

    fn list_plugins(&self) -> Vec<PluginInstance> {
        self.instances.iter().map(|e| e.value().clone()).collect()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest() -> PluginManifest {
        PluginManifest {
            name: "test-technique".to_string(),
            version: "1.0.0".to_string(),
            description: "A test imported technique".to_string(),
            author: "Sensei Test".to_string(),
            entry_point: "execute".to_string(),
            capabilities: vec!["read".to_string()],
            max_memory_bytes: default_max_memory(),
            max_execution_ms: default_max_execution_ms(),
            permissions: PluginPermissions::default(),
        }
    }

    fn echo_output(input: &PluginInput) -> PluginOutput {
        PluginOutput {
            result: input.args.clone(),
            logs: vec!["executed".to_string()],
            execution_ms: 0,
            memory_used_bytes: 1024,
        }
    }

    // --- Manifest validation tests ---

    #[test]
    fn test_manifest_validation_valid() {
        let manifest = valid_manifest();
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
    }

    #[test]
    fn test_manifest_validation_empty_name() {
        let mut manifest = valid_manifest();
        manifest.name = "".to_string();
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_manifest_validation_empty_version() {
        let mut manifest = valid_manifest();
        manifest.version = "   ".to_string();
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.iter().any(|e| e.contains("version")));
    }

    // --- Registry tests ---

    #[tokio::test]
    async fn test_registry_register_plugin() {
        let registry = PluginRegistry::new();
        let id = registry.register(valid_manifest(), b"fake-wasm").await;
        assert!(id.is_ok());
        assert_eq!(registry.plugin_count(), 1);
    }

    #[tokio::test]
    async fn test_registry_list_plugins() {
        let registry = PluginRegistry::new();
        registry.register(valid_manifest(), b"fake").await.ok();

        let mut m2 = valid_manifest();
        m2.name = "second-technique".to_string();
        registry.register(m2, b"fake").await.ok();

        let plugins = registry.list();
        assert_eq!(plugins.len(), 2);
    }

    #[tokio::test]
    async fn test_registry_get_by_name() {
        let registry = PluginRegistry::new();
        registry.register(valid_manifest(), b"fake").await.ok();

        let found = registry.get_by_name("test-technique");
        assert!(found.is_some());
        assert_eq!(
            found.as_ref().map(|p| p.manifest.name.as_str()),
            Some("test-technique")
        );

        let not_found = registry.get_by_name("nonexistent");
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_registry_get_by_id() {
        let registry = PluginRegistry::new();
        let id = registry
            .register(valid_manifest(), b"fake")
            .await
            .expect("register");

        let found = registry.get(&id);
        assert!(found.is_some());
        assert_eq!(found.as_ref().map(|p| p.id), Some(id));
    }

    #[tokio::test]
    async fn test_registry_unregister() {
        let registry = PluginRegistry::new();
        let id = registry
            .register(valid_manifest(), b"fake")
            .await
            .expect("register");
        assert_eq!(registry.plugin_count(), 1);

        registry.unregister(&id).await.expect("unregister");
        assert_eq!(registry.plugin_count(), 0);
    }

    #[tokio::test]
    async fn test_registry_plugin_count() {
        let registry = PluginRegistry::new();
        assert_eq!(registry.plugin_count(), 0);

        registry.register(valid_manifest(), b"fake").await.ok();
        assert_eq!(registry.plugin_count(), 1);
    }

    // --- Permissions tests ---

    #[test]
    fn test_default_permissions_restrictive() {
        let perms = PluginPermissions::default();
        assert!(!perms.allow_network);
        assert!(perms.allow_filesystem.is_empty());
        assert!(perms.allow_env_vars.is_empty());
        assert!(!perms.allow_subprocess);
    }

    // --- Plugin state tests ---

    #[test]
    fn test_plugin_instance_tracks_state() {
        let instance = PluginInstance {
            id: Uuid::new_v4(),
            manifest: valid_manifest(),
            state: PluginState::Loaded,
            loaded_at: Utc::now(),
            last_invoked: None,
            invocation_count: 0,
            total_execution_ms: 0,
        };
        assert_eq!(instance.state, PluginState::Loaded);

        let mut running = instance.clone();
        running.state = PluginState::Running;
        assert_eq!(running.state, PluginState::Running);

        let mut errored = instance.clone();
        errored.state = PluginState::Error("knocked out".to_string());
        assert_eq!(errored.state, PluginState::Error("knocked out".to_string()));
    }

    // --- NativePluginRuntime tests ---

    #[tokio::test]
    async fn test_native_runtime_register_and_invoke() {
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = PluginRegistry::with_runtime(runtime.clone());

        let id = registry
            .register(valid_manifest(), b"native")
            .await
            .expect("register");

        // Register a native function that echoes input
        runtime.register_function(id, |input| Ok(echo_output(&input)));

        let input = PluginInput {
            function: "execute".to_string(),
            args: serde_json::json!({"strike": "uppercut"}),
            context: serde_json::json!({}),
        };

        let output = registry.invoke(&id, input).await.expect("invoke");
        assert_eq!(output.result, serde_json::json!({"strike": "uppercut"}));
        assert!(!output.logs.is_empty());
    }

    #[tokio::test]
    async fn test_native_runtime_unload() {
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = PluginRegistry::with_runtime(runtime.clone());

        let id = registry
            .register(valid_manifest(), b"native")
            .await
            .expect("register");
        runtime.register_function(id, |input| Ok(echo_output(&input)));

        assert_eq!(registry.plugin_count(), 1);
        registry.unregister(&id).await.expect("unregister");
        assert_eq!(registry.plugin_count(), 0);

        // Runtime should also have removed the plugin
        assert!(runtime.list_plugins().is_empty());
    }

    #[test]
    fn test_manifest_validation_empty_entry_point() {
        let mut manifest = valid_manifest();
        manifest.entry_point = "  ".to_string();
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.iter().any(|e| e.contains("entry_point")));
    }

    #[test]
    fn test_manifest_validation_zero_memory() {
        let mut manifest = valid_manifest();
        manifest.max_memory_bytes = 0;
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.iter().any(|e| e.contains("max_memory_bytes")));
    }

    #[test]
    fn test_manifest_validation_zero_execution_ms() {
        let mut manifest = valid_manifest();
        manifest.max_execution_ms = 0;
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.iter().any(|e| e.contains("max_execution_ms")));
    }

    #[test]
    fn test_manifest_validation_multiple_errors() {
        let manifest = PluginManifest {
            name: "".to_string(),
            version: "".to_string(),
            description: "bad".to_string(),
            author: "x".to_string(),
            entry_point: "".to_string(),
            capabilities: vec![],
            max_memory_bytes: 0,
            max_execution_ms: 0,
            permissions: PluginPermissions::default(),
        };
        let errors = PluginRegistry::validate_manifest(&manifest);
        assert!(errors.len() >= 5, "should have multiple errors: {errors:?}");
    }

    #[test]
    fn test_manifest_serde_roundtrip() {
        let manifest = valid_manifest();
        let json = serde_json::to_string(&manifest).unwrap();
        let restored: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test-technique");
        assert_eq!(restored.version, "1.0.0");
    }

    #[test]
    fn test_plugin_state_equality() {
        assert_eq!(PluginState::Loaded, PluginState::Loaded);
        assert_ne!(PluginState::Loaded, PluginState::Running);
        assert_ne!(PluginState::Stopped, PluginState::Error("x".into()));
        assert_eq!(
            PluginState::Error("foo".into()),
            PluginState::Error("foo".into())
        );
    }

    #[tokio::test]
    async fn test_registry_reject_invalid_manifest() {
        let registry = PluginRegistry::new();
        let mut manifest = valid_manifest();
        manifest.name = "".to_string();
        let result = registry.register(manifest, b"fake").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_registry_default() {
        let registry = PluginRegistry::default();
        assert_eq!(registry.plugin_count(), 0);
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let registry = PluginRegistry::new();
        let id = Uuid::new_v4();
        let result = registry.unregister(&id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invoke_without_runtime() {
        let registry = PluginRegistry::new();
        let id = registry.register(valid_manifest(), b"fake").await.unwrap();
        let input = PluginInput {
            function: "execute".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };
        let result = registry.invoke(&id, input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_plugin_invocation_count_incremented() {
        let runtime = Arc::new(NativePluginRuntime::new());
        let registry = PluginRegistry::with_runtime(runtime.clone());

        let id = registry
            .register(valid_manifest(), b"native")
            .await
            .expect("register");
        runtime.register_function(id, |input| Ok(echo_output(&input)));

        let make_input = || PluginInput {
            function: "execute".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        registry.invoke(&id, make_input()).await.expect("invoke 1");
        registry.invoke(&id, make_input()).await.expect("invoke 2");
        registry.invoke(&id, make_input()).await.expect("invoke 3");

        let plugin = registry.get(&id).expect("plugin should exist");
        assert_eq!(plugin.invocation_count, 3);
        assert!(plugin.last_invoked.is_some());
    }
}
