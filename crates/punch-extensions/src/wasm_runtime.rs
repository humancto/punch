//! # WASM Plugin Runtime (wasmi)
//!
//! A real WebAssembly plugin runtime backed by the `wasmi` interpreter.
//! Plugins are compiled from WASM bytecode, sandboxed with fuel metering
//! and memory limits, and executed through the [`PluginRuntime`] trait.
//!
//! ## Host Functions
//!
//! The runtime exposes three host functions to guest modules:
//!
//! - `host_log(ptr, len)` — log a message from guest memory
//! - `host_read_input(ptr)` — write the JSON input into guest memory at `ptr`
//! - `host_write_output(ptr, len)` — read JSON output from guest memory

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use tracing::{debug, info};
use uuid::Uuid;
use wasmi::{
    Caller, Config, Engine, Extern, Func, Linker, Memory, MemoryType, Module, Store,
    StoreLimitsBuilder,
};

use punch_types::{PunchError, PunchResult};

use crate::plugin::{
    PluginInput, PluginInstance, PluginManifest, PluginOutput, PluginRuntime, PluginState,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the WASM plugin runtime.
#[derive(Debug, Clone)]
pub struct WasmRuntimeConfig {
    /// Maximum fuel (instruction budget) per function call.
    pub fuel_limit: u64,
    /// Maximum memory per instance in bytes (default: 16 MB).
    pub max_memory_bytes: usize,
}

impl Default for WasmRuntimeConfig {
    fn default() -> Self {
        Self {
            fuel_limit: 1_000_000,
            max_memory_bytes: 16 * 1024 * 1024, // 16 MB
        }
    }
}

// ---------------------------------------------------------------------------
// Host State — data shared between host and guest during execution
// ---------------------------------------------------------------------------

/// State stored inside each wasmi `Store`, accessible from host functions.
struct HostState {
    /// JSON input to pass to the guest.
    input_json: Vec<u8>,
    /// JSON output written by the guest.
    output_json: Vec<u8>,
    /// Log lines collected from guest `host_log` calls.
    logs: Vec<String>,
    /// Store limits for memory enforcement.
    limits: wasmi::StoreLimits,
}

// ---------------------------------------------------------------------------
// WasmPluginRuntime
// ---------------------------------------------------------------------------

/// A WebAssembly plugin runtime backed by the `wasmi` interpreter.
///
/// Each plugin is compiled into a [`wasmi::Module`] and stored in a concurrent
/// map. When a function is called, a fresh [`Store`] is created with fuel
/// metering and memory limits, the module is instantiated, and the requested
/// export is invoked.
pub struct WasmPluginRuntime {
    /// The wasmi execution engine (shared across all modules).
    engine: Engine,
    /// Compiled modules keyed by plugin UUID.
    modules: DashMap<Uuid, Module>,
    /// Plugin instance metadata keyed by plugin UUID.
    instances: DashMap<Uuid, PluginInstance>,
    /// Runtime configuration.
    config: WasmRuntimeConfig,
}

impl WasmPluginRuntime {
    /// Create a new WASM plugin runtime with default configuration.
    pub fn new() -> PunchResult<Self> {
        Self::with_config(WasmRuntimeConfig::default())
    }

    /// Create a new WASM plugin runtime with custom configuration.
    pub fn with_config(config: WasmRuntimeConfig) -> PunchResult<Self> {
        let mut engine_config = Config::default();
        engine_config.consume_fuel(true);

        let engine = Engine::new(&engine_config);

        Ok(Self {
            engine,
            modules: DashMap::new(),
            instances: DashMap::new(),
            config,
        })
    }

    /// Compile WASM bytes into a module without registering it.
    ///
    /// Useful for validation. Returns the compiled module or an error if the
    /// bytes are not valid WebAssembly.
    pub fn compile(&self, wasm_bytes: &[u8]) -> PunchResult<Module> {
        Module::new(&self.engine, wasm_bytes)
            .map_err(|e| PunchError::Internal(format!("WASM compilation failed: {e}")))
    }

    /// Return the number of loaded plugins.
    pub fn plugin_count(&self) -> usize {
        self.modules.len()
    }

    /// Check whether a plugin with the given ID is loaded.
    pub fn plugin_exists(&self, plugin_id: &Uuid) -> bool {
        self.modules.contains_key(plugin_id)
    }

    /// Return the names of all loaded plugins.
    pub fn plugin_names(&self) -> Vec<String> {
        self.instances
            .iter()
            .map(|e| e.value().manifest.name.clone())
            .collect()
    }

    /// Create a [`Store`] with fuel and memory limits applied.
    fn create_store(&self, input_json: Vec<u8>) -> Store<HostState> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.config.max_memory_bytes)
            .build();

        let host_state = HostState {
            input_json,
            output_json: Vec::new(),
            logs: Vec::new(),
            limits,
        };

        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);

        // Set the fuel budget for this invocation.
        store
            .set_fuel(self.config.fuel_limit)
            .expect("fuel metering is enabled");

        store
    }

    /// Build a linker with host functions and a shared memory export.
    fn build_linker(&self, store: &mut Store<HostState>) -> PunchResult<Linker<HostState>> {
        let mut linker = <Linker<HostState>>::new(&self.engine);

        // Provide a default memory if the guest module imports one.
        // 1 page = 64 KiB. Max pages based on config.
        let max_pages = (self.config.max_memory_bytes / 65536) as u32;
        let memory_type = MemoryType::new(1, Some(max_pages))
            .map_err(|e| PunchError::Internal(format!("failed to create memory type: {e}")))?;
        let memory = Memory::new(&mut *store, memory_type)
            .map_err(|e| PunchError::Internal(format!("failed to create memory: {e}")))?;
        linker
            .define("env", "memory", memory)
            .map_err(|e| PunchError::Internal(format!("failed to define memory: {e}")))?;

        // host_log(ptr: i32, len: i32)
        let host_log = Func::wrap(
            &mut *store,
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let mem = caller.get_export("memory").and_then(Extern::into_memory);
                if let Some(mem) = mem {
                    let start = ptr as usize;
                    let end = start + len as usize;
                    let data = mem.data(&caller);
                    if end <= data.len() {
                        let msg = String::from_utf8_lossy(&data[start..end]).to_string();
                        debug!(%msg, "guest log");
                        caller.data_mut().logs.push(msg);
                    }
                }
            },
        );
        linker
            .define("env", "host_log", host_log)
            .map_err(|e| PunchError::Internal(format!("failed to define host_log: {e}")))?;

        // host_read_input(ptr: i32) -> i32 (returns length written)
        let host_read_input = Func::wrap(
            &mut *store,
            |mut caller: Caller<'_, HostState>, ptr: i32| -> i32 {
                let input_copy = caller.data().input_json.clone();
                let mem = caller.get_export("memory").and_then(Extern::into_memory);
                if let Some(mem) = mem {
                    let start = ptr as usize;
                    let len = input_copy.len();
                    if mem.data(&caller).len() >= start + len {
                        mem.data_mut(&mut caller)[start..start + len].copy_from_slice(&input_copy);
                        len as i32
                    } else {
                        -1 // not enough memory
                    }
                } else {
                    -1
                }
            },
        );
        linker
            .define("env", "host_read_input", host_read_input)
            .map_err(|e| PunchError::Internal(format!("failed to define host_read_input: {e}")))?;

        // host_write_output(ptr: i32, len: i32)
        let host_write_output = Func::wrap(
            &mut *store,
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let mem = caller.get_export("memory").and_then(Extern::into_memory);
                if let Some(mem) = mem {
                    let start = ptr as usize;
                    let end = start + len as usize;
                    let data = mem.data(&caller);
                    if end <= data.len() {
                        let output = data[start..end].to_vec();
                        caller.data_mut().output_json = output;
                    }
                }
            },
        );
        linker
            .define("env", "host_write_output", host_write_output)
            .map_err(|e| {
                PunchError::Internal(format!("failed to define host_write_output: {e}"))
            })?;

        Ok(linker)
    }
}

impl Default for WasmPluginRuntime {
    fn default() -> Self {
        Self::new().expect("failed to create default WASM runtime")
    }
}

// ---------------------------------------------------------------------------
// PluginRuntime implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PluginRuntime for WasmPluginRuntime {
    async fn load(&self, manifest: &PluginManifest, wasm_bytes: &[u8]) -> PunchResult<Uuid> {
        let module = self.compile(wasm_bytes)?;
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

        self.modules.insert(id, module);
        self.instances.insert(id, instance);

        info!(
            plugin_id = %id,
            name = %manifest.name,
            "loaded WASM technique into runtime"
        );

        Ok(id)
    }

    async fn invoke(&self, plugin_id: &Uuid, input: PluginInput) -> PunchResult<PluginOutput> {
        let module_ref = self
            .modules
            .get(plugin_id)
            .ok_or_else(|| PunchError::Internal(format!("WASM plugin {plugin_id} not found")))?;
        let module = module_ref.value().clone();
        drop(module_ref); // release DashMap guard before doing work

        let input_json = serde_json::to_vec(&input.args)
            .map_err(|e| PunchError::Internal(format!("failed to serialize input: {e}")))?;

        let start = std::time::Instant::now();

        let mut store = self.create_store(input_json);
        let linker = self.build_linker(&mut store)?;

        // Instantiate and start the module.
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| PunchError::Internal(format!("WASM instantiation failed: {e}")))?
            .start(&mut store)
            .map_err(|e| PunchError::Internal(format!("WASM start failed: {e}")))?;

        // Look up the requested export.
        let func = instance.get_func(&store, &input.function).ok_or_else(|| {
            PunchError::Internal(format!(
                "export '{}' not found in plugin {}",
                input.function, plugin_id
            ))
        })?;

        // Determine the number of return values from the function type.
        let func_type = func.ty(&store);
        let num_results = func_type.results().len();
        let mut results: Vec<wasmi::Val> = vec![wasmi::Val::I32(0); num_results];

        let call_result = func.call(&mut store, &[], &mut results);

        let execution_ms = start.elapsed().as_millis() as u64;

        // Check for errors, distinguishing fuel exhaustion.
        if let Err(ref e) = call_result {
            let err_str = e.to_string();
            if err_str.contains("fuel") {
                return Err(PunchError::Internal(format!(
                    "WASM execution exceeded fuel limit ({} units): {err_str}",
                    self.config.fuel_limit
                )));
            }
            return Err(PunchError::Internal(format!(
                "WASM function call failed: {err_str}"
            )));
        }

        // Extract output and logs from the store.
        let host_state = store.data();
        let output_json = host_state.output_json.clone();
        let logs = host_state.logs.clone();

        // Parse output JSON, or use the raw return value.
        let result = if !output_json.is_empty() {
            serde_json::from_slice(&output_json).unwrap_or(serde_json::Value::Null)
        } else if results.is_empty() {
            serde_json::Value::Null
        } else {
            match results[0] {
                wasmi::Val::I32(v) => serde_json::Value::Number(serde_json::Number::from(v)),
                wasmi::Val::I64(v) => serde_json::Value::Number(serde_json::Number::from(v)),
                _ => serde_json::Value::Null,
            }
        };

        // Update instance stats.
        if let Some(mut inst) = self.instances.get_mut(plugin_id) {
            inst.invocation_count += 1;
            inst.total_execution_ms += execution_ms;
            inst.last_invoked = Some(Utc::now());
        }

        Ok(PluginOutput {
            result,
            logs,
            execution_ms,
            memory_used_bytes: 0, // wasmi does not expose precise memory tracking
        })
    }

    async fn unload(&self, plugin_id: &Uuid) -> PunchResult<()> {
        self.modules.remove(plugin_id);
        self.instances.remove(plugin_id);
        info!(plugin_id = %plugin_id, "unloaded WASM technique from runtime");
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
    use std::sync::Arc;

    use crate::plugin::{PluginManifest, PluginPermissions, PluginRegistry};

    fn test_manifest(name: &str) -> PluginManifest {
        PluginManifest {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "Test WASM technique".to_string(),
            author: "Test".to_string(),
            entry_point: "execute".to_string(),
            capabilities: vec![],
            max_memory_bytes: 16 * 1024 * 1024,
            max_execution_ms: 30_000,
            permissions: PluginPermissions::default(),
        }
    }

    /// Compile WAT text to WASM binary bytes.
    fn wat_to_wasm(wat: &str) -> Vec<u8> {
        wat::parse_str(wat).expect("failed to parse WAT")
    }

    // --- Test 1: Engine creation ---

    #[test]
    fn test_engine_creation() {
        let runtime = WasmPluginRuntime::new();
        assert!(runtime.is_ok(), "engine creation should succeed");
        assert_eq!(runtime.unwrap().plugin_count(), 0);
    }

    #[test]
    fn test_engine_creation_with_custom_config() {
        let config = WasmRuntimeConfig {
            fuel_limit: 500_000,
            max_memory_bytes: 8 * 1024 * 1024,
        };
        let runtime = WasmPluginRuntime::with_config(config);
        assert!(runtime.is_ok());
    }

    // --- Test 2: Module compilation from valid WAT ---

    #[test]
    fn test_compile_valid_wat() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(
            r#"(module
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add)
            )"#,
        );
        let result = runtime.compile(&wasm);
        assert!(result.is_ok(), "valid WAT should compile: {result:?}");
    }

    // --- Test 3: Function call with input/output ---

    #[tokio::test]
    async fn test_function_call_returns_value() {
        let runtime = WasmPluginRuntime::new().unwrap();

        // A simple module that exports a function returning a constant.
        let wasm = wat_to_wasm(
            r#"(module
                (func (export "answer") (result i32)
                    i32.const 42)
            )"#,
        );

        let manifest = test_manifest("answer-plugin");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        let input = PluginInput {
            function: "answer".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let output = runtime.invoke(&id, input).await.unwrap();
        assert_eq!(output.result, serde_json::json!(42));
    }

    // --- Test 4: Fuel exhaustion handling ---

    #[tokio::test]
    async fn test_fuel_exhaustion() {
        let config = WasmRuntimeConfig {
            fuel_limit: 100, // very low fuel
            max_memory_bytes: 16 * 1024 * 1024,
        };
        let runtime = WasmPluginRuntime::with_config(config).unwrap();

        // An infinite loop module — will exhaust fuel.
        let wasm = wat_to_wasm(
            r#"(module
                (func (export "loop_forever")
                    (loop $inf
                        br $inf))
            )"#,
        );

        let manifest = test_manifest("infinite-loop");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        let input = PluginInput {
            function: "loop_forever".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let result = runtime.invoke(&id, input).await;
        assert!(
            result.is_err(),
            "infinite loop should fail with fuel exhaustion"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("fuel"),
            "error should mention fuel: {err_msg}"
        );
    }

    // --- Test 5: Invalid WASM rejection ---

    #[test]
    fn test_invalid_wasm_rejected() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let invalid_bytes = b"this is not valid wasm";
        let result = runtime.compile(invalid_bytes);
        assert!(result.is_err(), "invalid bytes should fail compilation");
    }

    #[tokio::test]
    async fn test_load_invalid_wasm_rejected() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let manifest = test_manifest("bad-plugin");
        let result = runtime.load(&manifest, b"not wasm").await;
        assert!(result.is_err());
    }

    // --- Test 6: Plugin loading/unloading lifecycle ---

    #[tokio::test]
    async fn test_plugin_lifecycle() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(r#"(module (func (export "noop")))"#);

        // Load
        let manifest = test_manifest("lifecycle-plugin");
        let id = runtime.load(&manifest, &wasm).await.unwrap();
        assert!(runtime.plugin_exists(&id));
        assert_eq!(runtime.plugin_count(), 1);

        // Unload
        runtime.unload(&id).await.unwrap();
        assert!(!runtime.plugin_exists(&id));
        assert_eq!(runtime.plugin_count(), 0);
    }

    // --- Test 7: List plugins ---

    #[tokio::test]
    async fn test_list_plugins() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(r#"(module (func (export "noop")))"#);

        runtime
            .load(&test_manifest("plugin-a"), &wasm)
            .await
            .unwrap();
        runtime
            .load(&test_manifest("plugin-b"), &wasm)
            .await
            .unwrap();

        let plugins = runtime.list_plugins();
        assert_eq!(plugins.len(), 2);

        let names: Vec<String> = plugins.iter().map(|p| p.manifest.name.clone()).collect();
        assert!(names.contains(&"plugin-a".to_string()));
        assert!(names.contains(&"plugin-b".to_string()));
    }

    #[tokio::test]
    async fn test_plugin_names() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(r#"(module (func (export "noop")))"#);

        runtime.load(&test_manifest("alpha"), &wasm).await.unwrap();
        runtime.load(&test_manifest("beta"), &wasm).await.unwrap();

        let names = runtime.plugin_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
    }

    // --- Test 8: Memory limit enforcement ---

    #[tokio::test]
    async fn test_memory_limit_enforcement() {
        let config = WasmRuntimeConfig {
            fuel_limit: 10_000_000,
            // Allow only 1 page (64 KiB) of memory.
            max_memory_bytes: 65536,
        };
        let runtime = WasmPluginRuntime::with_config(config).unwrap();

        // Module that declares 2 pages of memory minimum — should fail
        // because our limit only allows 1 page.
        let wasm = wat_to_wasm(
            r#"(module
                (memory (export "memory") 2)
                (func (export "noop"))
            )"#,
        );

        let manifest = test_manifest("memory-hog");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        let input = PluginInput {
            function: "noop".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let result = runtime.invoke(&id, input).await;
        assert!(
            result.is_err(),
            "should fail due to memory limits: {result:?}"
        );
    }

    // --- Test 9: Multiple plugins simultaneously ---

    #[tokio::test]
    async fn test_multiple_plugins_simultaneously() {
        let runtime = Arc::new(WasmPluginRuntime::new().unwrap());

        let wasm_add = wat_to_wasm(
            r#"(module
                (func (export "compute") (result i32) i32.const 10)
            )"#,
        );
        let wasm_sub = wat_to_wasm(
            r#"(module
                (func (export "compute") (result i32) i32.const 20)
            )"#,
        );

        let id_a = runtime
            .load(&test_manifest("adder"), &wasm_add)
            .await
            .unwrap();
        let id_b = runtime
            .load(&test_manifest("subtractor"), &wasm_sub)
            .await
            .unwrap();

        let make_input = || PluginInput {
            function: "compute".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let out_a = runtime.invoke(&id_a, make_input()).await.unwrap();
        let out_b = runtime.invoke(&id_b, make_input()).await.unwrap();

        assert_eq!(out_a.result, serde_json::json!(10));
        assert_eq!(out_b.result, serde_json::json!(20));
    }

    // --- Test 10: Missing export handling ---

    #[tokio::test]
    async fn test_missing_export() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(
            r#"(module
                (func (export "existing_func") (result i32) i32.const 1)
            )"#,
        );

        let manifest = test_manifest("sparse-plugin");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        let input = PluginInput {
            function: "nonexistent_func".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let result = runtime.invoke(&id, input).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found"),
            "error should mention missing export: {err_msg}"
        );
    }

    // --- Test 11: Plugin not found ---

    #[tokio::test]
    async fn test_invoke_nonexistent_plugin() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let fake_id = Uuid::new_v4();

        let input = PluginInput {
            function: "anything".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let result = runtime.invoke(&fake_id, input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // --- Test 12: Integration with PluginRegistry ---

    #[tokio::test]
    async fn test_registry_with_wasm_runtime() {
        let runtime = Arc::new(WasmPluginRuntime::new().unwrap());
        let registry = PluginRegistry::with_runtime(runtime);

        let wasm = wat_to_wasm(
            r#"(module
                (func (export "execute") (result i32) i32.const 99)
            )"#,
        );

        let manifest = test_manifest("registry-test");
        let id = registry.register(manifest, &wasm).await.unwrap();
        assert_eq!(registry.plugin_count(), 1);

        let input = PluginInput {
            function: "execute".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let output = registry.invoke(&id, input).await.unwrap();
        assert_eq!(output.result, serde_json::json!(99));

        // Verify stats are updated.
        let plugin = registry.get(&id).unwrap();
        assert_eq!(plugin.invocation_count, 1);
    }

    // --- Test 13: Invocation count tracking ---

    #[tokio::test]
    async fn test_invocation_count_tracking() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(r#"(module (func (export "tick") (result i32) i32.const 1))"#);

        let manifest = test_manifest("counter-plugin");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        for _ in 0..5 {
            let input = PluginInput {
                function: "tick".to_string(),
                args: serde_json::json!({}),
                context: serde_json::json!({}),
            };
            runtime.invoke(&id, input).await.unwrap();
        }

        let plugins = runtime.list_plugins();
        let plugin = plugins.iter().find(|p| p.id == id).unwrap();
        assert_eq!(plugin.invocation_count, 5);
        assert!(plugin.last_invoked.is_some());
    }

    // --- Test 14: Module with i64 return ---

    #[tokio::test]
    async fn test_i64_return_value() {
        let runtime = WasmPluginRuntime::new().unwrap();
        let wasm = wat_to_wasm(
            r#"(module
                (func (export "big") (result i64) i64.const 9999999999)
            )"#,
        );

        let manifest = test_manifest("big-numbers");
        let id = runtime.load(&manifest, &wasm).await.unwrap();

        let input = PluginInput {
            function: "big".to_string(),
            args: serde_json::json!({}),
            context: serde_json::json!({}),
        };

        let output = runtime.invoke(&id, input).await.unwrap();
        assert_eq!(output.result, serde_json::json!(9_999_999_999_i64));
    }
}
