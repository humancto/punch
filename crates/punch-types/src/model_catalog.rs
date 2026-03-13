//! # Model Catalog — The Fighter Roster
//!
//! A comprehensive catalog of available models across all providers, tracking
//! capabilities, pricing, context windows, and usage statistics. Think of it as
//! the official fighter roster: every contender registered, weighed in, and
//! categorized by weight class before they step into the ring.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::config::Provider;
use crate::fighter::WeightClass;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// What a model can do in the ring — its signature moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    /// Standard text generation / chat completion.
    TextGeneration,
    /// Function-calling / tool use.
    ToolUse,
    /// Image understanding and multimodal input.
    Vision,
    /// Optimized for code generation and editing.
    CodeGeneration,
    /// Produces embedding vectors.
    Embedding,
    /// Extended chain-of-thought / reasoning mode.
    Reasoning,
    /// Supports streaming token delivery.
    Streaming,
}

/// Pricing card for a model — how much it costs to throw punches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// USD per 1 million input tokens.
    pub input_per_million: f64,
    /// USD per 1 million output tokens.
    pub output_per_million: f64,
    /// USD per 1 million cached/prompt-cached input tokens, if supported.
    pub cached_input_per_million: Option<f64>,
}

/// Full profile of a model — its fight card entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Unique model identifier (e.g. `"anthropic/claude-sonnet-4-20250514"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// The provider that hosts this model.
    pub provider: Provider,
    /// Maximum context window in tokens.
    pub context_window: u32,
    /// Maximum output tokens per request.
    pub max_output_tokens: u32,
    /// The model's signature moves.
    pub capabilities: Vec<ModelCapability>,
    /// Cost info (None for free / local models).
    pub pricing: Option<ModelPricing>,
    /// Weight class categorization for this model.
    pub weight_class: WeightClass,
    /// Whether the model is currently available to fight.
    pub is_available: bool,
    /// Additional provider-specific metadata.
    pub metadata: serde_json::Value,
}

/// Cumulative usage statistics for a model — its fight record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageStats {
    /// Model this record belongs to.
    pub model_id: String,
    /// Total requests dispatched.
    pub total_requests: u64,
    /// Total input tokens consumed.
    pub total_input_tokens: u64,
    /// Total output tokens generated.
    pub total_output_tokens: u64,
    /// Total errored requests.
    pub total_errors: u64,
    /// Running average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Timestamp of the most recent use.
    pub last_used: Option<DateTime<Utc>>,
    /// Estimated cumulative cost in USD.
    pub estimated_cost_usd: f64,
}

impl ModelUsageStats {
    fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            total_requests: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_errors: 0,
            avg_latency_ms: 0.0,
            last_used: None,
            estimated_cost_usd: 0.0,
        }
    }
}

/// Requirements for selecting a model — the matchmaker's criteria.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRequirements {
    /// Minimum context window the model must support.
    pub min_context_window: Option<u32>,
    /// Capabilities the model must possess.
    pub required_capabilities: Vec<ModelCapability>,
    /// Maximum acceptable cost per 1 million output tokens (USD).
    pub max_cost_per_million_output: Option<f64>,
    /// Preferred provider (gives priority but does not exclude others).
    pub preferred_provider: Option<Provider>,
    /// Preferred weight class.
    pub preferred_weight_class: Option<WeightClass>,
}

// ---------------------------------------------------------------------------
// The Catalog
// ---------------------------------------------------------------------------

/// The Model Catalog — a thread-safe fighter roster that tracks every model
/// available across all providers, complete with weight class categorization,
/// capability profiles, pricing, and usage statistics.
#[derive(Debug)]
pub struct ModelCatalog {
    /// Registered models keyed by their unique ID.
    models: DashMap<String, ModelInfo>,
    /// Cumulative usage statistics per model.
    usage: DashMap<String, ModelUsageStats>,
    /// Short-name aliases that resolve to a canonical model ID.
    aliases: DashMap<String, String>,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelCatalog {
    /// Create an empty catalog — no fighters on the roster yet.
    pub fn new() -> Self {
        Self {
            models: DashMap::new(),
            usage: DashMap::new(),
            aliases: DashMap::new(),
        }
    }

    /// Create a catalog pre-populated with known models from major providers.
    /// The roster comes loaded and ready for fight night.
    pub fn with_builtin_models() -> Self {
        let catalog = Self::new();
        for model in builtin_models() {
            catalog.register_model(model);
        }
        catalog
    }

    /// Register a new model on the roster.
    pub fn register_model(&self, model: ModelInfo) {
        self.models.insert(model.id.clone(), model);
    }

    /// Look up a model by its ID or alias. Returns `None` if the fighter
    /// is not on the roster.
    pub fn get_model(&self, id: &str) -> Option<ModelInfo> {
        if let Some(entry) = self.models.get(id) {
            return Some(entry.value().clone());
        }
        // Try alias resolution.
        if let Some(canonical) = self.aliases.get(id)
            && let Some(entry) = self.models.get(canonical.value().as_str())
        {
            return Some(entry.value().clone());
        }
        None
    }

    /// Resolve an alias to its canonical model ID.
    pub fn resolve_alias(&self, alias: &str) -> Option<String> {
        self.aliases.get(alias).map(|r| r.value().clone())
    }

    /// Register a short-name alias that maps to a canonical model ID.
    pub fn add_alias(&self, alias: &str, model_id: &str) {
        self.aliases.insert(alias.to_string(), model_id.to_string());
    }

    /// List every model on the roster.
    pub fn list_models(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|r| r.value().clone()).collect()
    }

    /// List models from a specific provider — the provider's stable of fighters.
    pub fn list_by_provider(&self, provider: &Provider) -> Vec<ModelInfo> {
        self.models
            .iter()
            .filter(|r| &r.value().provider == provider)
            .map(|r| r.value().clone())
            .collect()
    }

    /// List models that possess a given capability.
    pub fn list_by_capability(&self, capability: &ModelCapability) -> Vec<ModelInfo> {
        self.models
            .iter()
            .filter(|r| r.value().capabilities.contains(capability))
            .map(|r| r.value().clone())
            .collect()
    }

    /// List models in a specific weight class.
    pub fn list_by_weight_class(&self, class: &WeightClass) -> Vec<ModelInfo> {
        self.models
            .iter()
            .filter(|r| &r.value().weight_class == class)
            .map(|r| r.value().clone())
            .collect()
    }

    /// Record usage for a model — log the round's stats.
    pub fn record_usage(
        &self,
        model_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        latency_ms: u64,
        is_error: bool,
    ) {
        let mut entry = self
            .usage
            .entry(model_id.to_string())
            .or_insert_with(|| ModelUsageStats::new(model_id));

        let stats = entry.value_mut();
        stats.total_requests += 1;
        stats.total_input_tokens += input_tokens;
        stats.total_output_tokens += output_tokens;
        if is_error {
            stats.total_errors += 1;
        }

        // Running average: new_avg = old_avg + (new_value - old_avg) / n
        let n = stats.total_requests as f64;
        stats.avg_latency_ms += (latency_ms as f64 - stats.avg_latency_ms) / n;

        stats.last_used = Some(Utc::now());

        // Update estimated cost if we have pricing data.
        if let Some(model) = self.models.get(model_id)
            && let Some(ref pricing) = model.value().pricing
        {
            let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million;
            let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
            stats.estimated_cost_usd += input_cost + output_cost;
        }
    }

    /// Get usage statistics for a model.
    pub fn get_usage(&self, model_id: &str) -> Option<ModelUsageStats> {
        self.usage.get(model_id).map(|r| r.value().clone())
    }

    /// Recommend the best model for a set of requirements — the matchmaker
    /// picks the ideal fighter for the bout.
    ///
    /// Scoring heuristic:
    /// - Models that fail hard requirements (context window, capabilities, cost) are excluded.
    /// - Preferred provider match: +10 points.
    /// - Preferred weight class match: +10 points.
    /// - Higher context window: +1 point per 100k tokens.
    /// - Available models only.
    pub fn recommend_model(&self, requirements: &ModelRequirements) -> Option<ModelInfo> {
        let mut best: Option<(ModelInfo, i64)> = None;

        for entry in self.models.iter() {
            let model = entry.value();

            if !model.is_available {
                continue;
            }

            // Hard requirement: minimum context window.
            if let Some(min_ctx) = requirements.min_context_window
                && model.context_window < min_ctx
            {
                continue;
            }

            // Hard requirement: all required capabilities must be present.
            let has_all_caps = requirements
                .required_capabilities
                .iter()
                .all(|c| model.capabilities.contains(c));
            if !has_all_caps {
                continue;
            }

            // Hard requirement: cost ceiling.
            if let Some(max_cost) = requirements.max_cost_per_million_output
                && let Some(ref pricing) = model.pricing
                && pricing.output_per_million > max_cost
            {
                continue;
            }

            // Scoring.
            let mut score: i64 = 0;

            if let Some(ref pref_provider) = requirements.preferred_provider
                && &model.provider == pref_provider
            {
                score += 10;
            }

            if let Some(ref pref_class) = requirements.preferred_weight_class
                && &model.weight_class == pref_class
            {
                score += 10;
            }

            // Bonus for larger context window.
            score += (model.context_window / 100_000) as i64;

            match &best {
                Some((_, best_score)) if score <= *best_score => {}
                _ => {
                    best = Some((model.clone(), score));
                }
            }
        }

        best.map(|(model, _)| model)
    }

    /// Total number of models on the roster.
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

// ---------------------------------------------------------------------------
// Builtin models — the pre-registered roster
// ---------------------------------------------------------------------------

/// Returns the pre-configured roster of well-known models from major providers.
fn builtin_models() -> Vec<ModelInfo> {
    let standard_chat_caps = vec![
        ModelCapability::TextGeneration,
        ModelCapability::ToolUse,
        ModelCapability::CodeGeneration,
        ModelCapability::Streaming,
    ];

    let vision_chat_caps = vec![
        ModelCapability::TextGeneration,
        ModelCapability::ToolUse,
        ModelCapability::Vision,
        ModelCapability::CodeGeneration,
        ModelCapability::Streaming,
    ];

    let reasoning_caps = vec![
        ModelCapability::TextGeneration,
        ModelCapability::Reasoning,
        ModelCapability::CodeGeneration,
    ];

    vec![
        // ----- Anthropic -------------------------------------------------
        ModelInfo {
            id: "anthropic/claude-opus-4-20250514".into(),
            name: "Claude Opus 4".into(),
            provider: Provider::Anthropic,
            context_window: 200_000,
            max_output_tokens: 32_000,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 15.0,
                output_per_million: 75.0,
                cached_input_per_million: Some(1.5),
            }),
            weight_class: WeightClass::Champion,
            is_available: true,
            metadata: serde_json::json!({"family": "claude-4"}),
        },
        ModelInfo {
            id: "anthropic/claude-sonnet-4-20250514".into(),
            name: "Claude Sonnet 4".into(),
            provider: Provider::Anthropic,
            context_window: 200_000,
            max_output_tokens: 16_000,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cached_input_per_million: Some(0.3),
            }),
            weight_class: WeightClass::Middleweight,
            is_available: true,
            metadata: serde_json::json!({"family": "claude-4"}),
        },
        ModelInfo {
            id: "anthropic/claude-haiku-3-5-20241022".into(),
            name: "Claude 3.5 Haiku".into(),
            provider: Provider::Anthropic,
            context_window: 200_000,
            max_output_tokens: 8_192,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 0.80,
                output_per_million: 4.0,
                cached_input_per_million: Some(0.08),
            }),
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "claude-3.5"}),
        },
        // ----- OpenAI ----------------------------------------------------
        ModelInfo {
            id: "openai/gpt-4o".into(),
            name: "GPT-4o".into(),
            provider: Provider::OpenAI,
            context_window: 128_000,
            max_output_tokens: 16_384,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 2.50,
                output_per_million: 10.0,
                cached_input_per_million: Some(1.25),
            }),
            weight_class: WeightClass::Middleweight,
            is_available: true,
            metadata: serde_json::json!({"family": "gpt-4o"}),
        },
        ModelInfo {
            id: "openai/gpt-4o-mini".into(),
            name: "GPT-4o Mini".into(),
            provider: Provider::OpenAI,
            context_window: 128_000,
            max_output_tokens: 16_384,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 0.15,
                output_per_million: 0.60,
                cached_input_per_million: Some(0.075),
            }),
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "gpt-4o"}),
        },
        ModelInfo {
            id: "openai/o1".into(),
            name: "OpenAI o1".into(),
            provider: Provider::OpenAI,
            context_window: 200_000,
            max_output_tokens: 100_000,
            capabilities: reasoning_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 15.0,
                output_per_million: 60.0,
                cached_input_per_million: Some(7.5),
            }),
            weight_class: WeightClass::Champion,
            is_available: true,
            metadata: serde_json::json!({"family": "o-series"}),
        },
        ModelInfo {
            id: "openai/o3-mini".into(),
            name: "OpenAI o3-mini".into(),
            provider: Provider::OpenAI,
            context_window: 200_000,
            max_output_tokens: 100_000,
            capabilities: reasoning_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 1.10,
                output_per_million: 4.40,
                cached_input_per_million: Some(0.55),
            }),
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "o-series"}),
        },
        // ----- Google ----------------------------------------------------
        ModelInfo {
            id: "google/gemini-2.5-pro".into(),
            name: "Gemini 2.5 Pro".into(),
            provider: Provider::Google,
            context_window: 1_000_000,
            max_output_tokens: 65_536,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 1.25,
                output_per_million: 10.0,
                cached_input_per_million: Some(0.315),
            }),
            weight_class: WeightClass::Heavyweight,
            is_available: true,
            metadata: serde_json::json!({"family": "gemini-2.5"}),
        },
        ModelInfo {
            id: "google/gemini-2.5-flash".into(),
            name: "Gemini 2.5 Flash".into(),
            provider: Provider::Google,
            context_window: 1_000_000,
            max_output_tokens: 65_536,
            capabilities: vision_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 0.15,
                output_per_million: 0.60,
                cached_input_per_million: Some(0.0375),
            }),
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "gemini-2.5"}),
        },
        // ----- DeepSeek --------------------------------------------------
        ModelInfo {
            id: "deepseek/deepseek-chat".into(),
            name: "DeepSeek Chat (V3)".into(),
            provider: Provider::DeepSeek,
            context_window: 64_000,
            max_output_tokens: 8_192,
            capabilities: standard_chat_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 0.27,
                output_per_million: 1.10,
                cached_input_per_million: Some(0.07),
            }),
            weight_class: WeightClass::Middleweight,
            is_available: true,
            metadata: serde_json::json!({"family": "deepseek-v3"}),
        },
        ModelInfo {
            id: "deepseek/deepseek-reasoner".into(),
            name: "DeepSeek Reasoner (R1)".into(),
            provider: Provider::DeepSeek,
            context_window: 64_000,
            max_output_tokens: 8_192,
            capabilities: reasoning_caps.clone(),
            pricing: Some(ModelPricing {
                input_per_million: 0.55,
                output_per_million: 2.19,
                cached_input_per_million: Some(0.14),
            }),
            weight_class: WeightClass::Heavyweight,
            is_available: true,
            metadata: serde_json::json!({"family": "deepseek-r1"}),
        },
        // ----- Ollama (local) --------------------------------------------
        ModelInfo {
            id: "ollama/llama3.1-8b".into(),
            name: "Llama 3.1 8B (Local)".into(),
            provider: Provider::Ollama,
            context_window: 128_000,
            max_output_tokens: 8_192,
            capabilities: standard_chat_caps.clone(),
            pricing: None,
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "llama-3.1", "local": true}),
        },
        ModelInfo {
            id: "ollama/llama3.1-70b".into(),
            name: "Llama 3.1 70B (Local)".into(),
            provider: Provider::Ollama,
            context_window: 128_000,
            max_output_tokens: 8_192,
            capabilities: standard_chat_caps.clone(),
            pricing: None,
            weight_class: WeightClass::Middleweight,
            is_available: true,
            metadata: serde_json::json!({"family": "llama-3.1", "local": true}),
        },
        ModelInfo {
            id: "ollama/mistral-7b".into(),
            name: "Mistral 7B (Local)".into(),
            provider: Provider::Ollama,
            context_window: 32_000,
            max_output_tokens: 4_096,
            capabilities: vec![
                ModelCapability::TextGeneration,
                ModelCapability::CodeGeneration,
                ModelCapability::Streaming,
            ],
            pricing: None,
            weight_class: WeightClass::Featherweight,
            is_available: true,
            metadata: serde_json::json!({"family": "mistral", "local": true}),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_model(id: &str, provider: Provider, weight: WeightClass) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            provider,
            context_window: 128_000,
            max_output_tokens: 4_096,
            capabilities: vec![ModelCapability::TextGeneration, ModelCapability::Streaming],
            pricing: Some(ModelPricing {
                input_per_million: 1.0,
                output_per_million: 5.0,
                cached_input_per_million: None,
            }),
            weight_class: weight,
            is_available: true,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn register_and_retrieve_model() {
        let catalog = ModelCatalog::new();
        let model = make_test_model(
            "test/model-a",
            Provider::Anthropic,
            WeightClass::Middleweight,
        );
        catalog.register_model(model.clone());

        let retrieved = catalog.get_model("test/model-a");
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved.as_ref().map(|m| &m.id),
            Some(&"test/model-a".to_string())
        );
    }

    #[test]
    fn alias_resolution() {
        let catalog = ModelCatalog::new();
        let model = make_test_model(
            "anthropic/claude-sonnet-4-20250514",
            Provider::Anthropic,
            WeightClass::Middleweight,
        );
        catalog.register_model(model);
        catalog.add_alias("sonnet", "anthropic/claude-sonnet-4-20250514");

        let resolved = catalog.resolve_alias("sonnet");
        assert_eq!(
            resolved,
            Some("anthropic/claude-sonnet-4-20250514".to_string())
        );

        // get_model should also work via alias.
        let via_alias = catalog.get_model("sonnet");
        assert!(via_alias.is_some());
        assert_eq!(
            via_alias.as_ref().map(|m| &m.id),
            Some(&"anthropic/claude-sonnet-4-20250514".to_string())
        );
    }

    #[test]
    fn list_by_provider() {
        let catalog = ModelCatalog::new();
        catalog.register_model(make_test_model(
            "a/m1",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        catalog.register_model(make_test_model(
            "o/m1",
            Provider::OpenAI,
            WeightClass::Middleweight,
        ));
        catalog.register_model(make_test_model(
            "a/m2",
            Provider::Anthropic,
            WeightClass::Featherweight,
        ));

        let anthropic = catalog.list_by_provider(&Provider::Anthropic);
        assert_eq!(anthropic.len(), 2);

        let openai = catalog.list_by_provider(&Provider::OpenAI);
        assert_eq!(openai.len(), 1);
    }

    #[test]
    fn list_by_capability() {
        let catalog = ModelCatalog::new();
        let mut model = make_test_model(
            "test/vision",
            Provider::Anthropic,
            WeightClass::Middleweight,
        );
        model.capabilities.push(ModelCapability::Vision);
        catalog.register_model(model);
        catalog.register_model(make_test_model(
            "test/no-vision",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));

        let vision_models = catalog.list_by_capability(&ModelCapability::Vision);
        assert_eq!(vision_models.len(), 1);
        assert_eq!(vision_models[0].id, "test/vision");
    }

    #[test]
    fn list_by_weight_class() {
        let catalog = ModelCatalog::new();
        catalog.register_model(make_test_model(
            "a",
            Provider::Anthropic,
            WeightClass::Featherweight,
        ));
        catalog.register_model(make_test_model(
            "b",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        catalog.register_model(make_test_model(
            "c",
            Provider::Anthropic,
            WeightClass::Featherweight,
        ));

        let feathers = catalog.list_by_weight_class(&WeightClass::Featherweight);
        assert_eq!(feathers.len(), 2);
    }

    #[test]
    fn usage_tracking_record_and_retrieve() {
        let catalog = ModelCatalog::new();
        catalog.register_model(make_test_model(
            "test/m",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        catalog.record_usage("test/m", 1000, 500, 200, false);

        let stats = catalog.get_usage("test/m");
        assert!(stats.is_some());
        let stats = stats.expect("stats should exist");
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.total_input_tokens, 1000);
        assert_eq!(stats.total_output_tokens, 500);
        assert_eq!(stats.total_errors, 0);
        assert!(stats.last_used.is_some());
    }

    #[test]
    fn usage_stats_accumulation() {
        let catalog = ModelCatalog::new();
        catalog.register_model(make_test_model(
            "test/m",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        catalog.record_usage("test/m", 1000, 500, 100, false);
        catalog.record_usage("test/m", 2000, 800, 300, false);
        catalog.record_usage("test/m", 500, 200, 200, true);

        let stats = catalog.get_usage("test/m").expect("stats should exist");
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.total_input_tokens, 3500);
        assert_eq!(stats.total_output_tokens, 1500);
        assert_eq!(stats.total_errors, 1);
        // Average latency should be (100 + 300 + 200) / 3 = 200
        assert!((stats.avg_latency_ms - 200.0).abs() < 1.0);
    }

    #[test]
    fn estimated_cost_calculation() {
        let catalog = ModelCatalog::new();
        // pricing: $1/M input, $5/M output
        catalog.register_model(make_test_model(
            "test/m",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        // 1M input tokens, 1M output tokens
        catalog.record_usage("test/m", 1_000_000, 1_000_000, 100, false);

        let stats = catalog.get_usage("test/m").expect("stats should exist");
        // Expected cost: (1M / 1M) * $1 + (1M / 1M) * $5 = $6.0
        assert!((stats.estimated_cost_usd - 6.0).abs() < 0.001);
    }

    #[test]
    fn model_recommendation_with_requirements() {
        let catalog = ModelCatalog::new();
        let mut big_ctx = make_test_model("big", Provider::Anthropic, WeightClass::Heavyweight);
        big_ctx.context_window = 500_000;
        catalog.register_model(big_ctx);

        let mut small_ctx = make_test_model("small", Provider::OpenAI, WeightClass::Featherweight);
        small_ctx.context_window = 8_000;
        catalog.register_model(small_ctx);

        let reqs = ModelRequirements {
            min_context_window: Some(100_000),
            ..Default::default()
        };

        let rec = catalog.recommend_model(&reqs);
        assert!(rec.is_some());
        assert_eq!(rec.as_ref().map(|m| &m.id), Some(&"big".to_string()));
    }

    #[test]
    fn recommendation_with_cost_constraint() {
        let catalog = ModelCatalog::new();

        let mut cheap = make_test_model("cheap", Provider::OpenAI, WeightClass::Featherweight);
        cheap.pricing = Some(ModelPricing {
            input_per_million: 0.1,
            output_per_million: 0.5,
            cached_input_per_million: None,
        });
        catalog.register_model(cheap);

        let mut expensive =
            make_test_model("expensive", Provider::Anthropic, WeightClass::Champion);
        expensive.pricing = Some(ModelPricing {
            input_per_million: 15.0,
            output_per_million: 75.0,
            cached_input_per_million: None,
        });
        catalog.register_model(expensive);

        let reqs = ModelRequirements {
            max_cost_per_million_output: Some(1.0),
            ..Default::default()
        };

        let rec = catalog.recommend_model(&reqs);
        assert!(rec.is_some());
        assert_eq!(rec.as_ref().map(|m| &m.id), Some(&"cheap".to_string()));
    }

    #[test]
    fn recommendation_with_provider_preference() {
        let catalog = ModelCatalog::new();
        catalog.register_model(make_test_model(
            "a/m",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        catalog.register_model(make_test_model(
            "o/m",
            Provider::OpenAI,
            WeightClass::Middleweight,
        ));

        let reqs = ModelRequirements {
            preferred_provider: Some(Provider::OpenAI),
            ..Default::default()
        };

        let rec = catalog.recommend_model(&reqs);
        assert!(rec.is_some());
        assert_eq!(
            rec.as_ref().map(|m| m.provider.clone()),
            Some(Provider::OpenAI)
        );
    }

    #[test]
    fn builtin_models_are_populated() {
        let catalog = ModelCatalog::with_builtin_models();
        assert!(catalog.model_count() > 0);

        // Spot-check a known model.
        let sonnet = catalog.get_model("anthropic/claude-sonnet-4-20250514");
        assert!(sonnet.is_some());
        let sonnet = sonnet.expect("sonnet should exist");
        assert_eq!(sonnet.provider, Provider::Anthropic);
        assert_eq!(sonnet.weight_class, WeightClass::Middleweight);
        assert!(sonnet.capabilities.contains(&ModelCapability::Vision));
    }

    #[test]
    fn builtin_model_count_is_reasonable() {
        let catalog = ModelCatalog::with_builtin_models();
        assert!(
            catalog.model_count() > 10,
            "expected more than 10 builtin models, got {}",
            catalog.model_count()
        );
    }

    #[test]
    fn unknown_model_returns_none() {
        let catalog = ModelCatalog::new();
        assert!(catalog.get_model("nonexistent/model").is_none());
        assert!(catalog.get_usage("nonexistent/model").is_none());
        assert!(catalog.resolve_alias("nonexistent").is_none());
    }

    #[test]
    fn serialization_round_trip() {
        let model = make_test_model(
            "test/roundtrip",
            Provider::Anthropic,
            WeightClass::Middleweight,
        );
        let json = serde_json::to_string(&model).expect("serialize should succeed");
        let deserialized: ModelInfo =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(deserialized.id, model.id);
        assert_eq!(deserialized.provider, model.provider);
        assert_eq!(deserialized.context_window, model.context_window);
        assert_eq!(deserialized.weight_class, model.weight_class);
    }

    #[test]
    fn model_count_reflects_registrations() {
        let catalog = ModelCatalog::new();
        assert_eq!(catalog.model_count(), 0);
        catalog.register_model(make_test_model(
            "a",
            Provider::Anthropic,
            WeightClass::Middleweight,
        ));
        assert_eq!(catalog.model_count(), 1);
        catalog.register_model(make_test_model(
            "b",
            Provider::OpenAI,
            WeightClass::Featherweight,
        ));
        assert_eq!(catalog.model_count(), 2);
    }

    #[test]
    fn usage_without_registered_model_still_records() {
        let catalog = ModelCatalog::new();
        // Record usage for a model that isn't registered — no pricing, but stats still accumulate.
        catalog.record_usage("ghost/model", 500, 250, 150, false);
        let stats = catalog
            .get_usage("ghost/model")
            .expect("stats should exist");
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.total_input_tokens, 500);
        // No pricing data means no cost estimate.
        assert!((stats.estimated_cost_usd - 0.0).abs() < 0.001);
    }
}
