//! Cost tracking and quota enforcement engine.
//!
//! The [`MeteringEngine`] calculates costs based on model pricing tables
//! and persists usage data through the memory substrate. It supports
//! per-fighter and aggregate spend queries across configurable time periods.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use punch_memory::MemorySubstrate;
use punch_types::{FighterId, PunchResult};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Pricing for a specific model (cost per million tokens).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    /// Cost per million input tokens (USD).
    pub input_per_million: f64,
    /// Cost per million output tokens (USD).
    pub output_per_million: f64,
}

/// Time period for spend queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendPeriod {
    Hour,
    Day,
    Month,
}

impl SpendPeriod {
    /// Convert to a chrono [`Duration`].
    fn to_duration(self) -> Duration {
        match self {
            Self::Hour => Duration::hours(1),
            Self::Day => Duration::days(1),
            Self::Month => Duration::days(30),
        }
    }
}

impl std::fmt::Display for SpendPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hour => write!(f, "hour"),
            Self::Day => write!(f, "day"),
            Self::Month => write!(f, "month"),
        }
    }
}

// ---------------------------------------------------------------------------
// MeteringEngine
// ---------------------------------------------------------------------------

/// Engine for tracking LLM costs and enforcing spend quotas.
pub struct MeteringEngine {
    /// Shared memory substrate for persisting usage data.
    memory: Arc<MemorySubstrate>,
    /// Pricing table keyed by model name (or prefix for wildcard matches).
    model_prices: HashMap<String, ModelPrice>,
}

impl MeteringEngine {
    /// Create a new metering engine with embedded default pricing.
    pub fn new(memory: Arc<MemorySubstrate>) -> Self {
        let model_prices = Self::default_price_table();
        Self {
            memory,
            model_prices,
        }
    }

    /// Create a new metering engine with custom pricing.
    pub fn with_prices(
        memory: Arc<MemorySubstrate>,
        model_prices: HashMap<String, ModelPrice>,
    ) -> Self {
        Self {
            memory,
            model_prices,
        }
    }

    /// Build the default embedded price table.
    fn default_price_table() -> HashMap<String, ModelPrice> {
        let mut prices = HashMap::new();

        prices.insert(
            "claude-opus".to_string(),
            ModelPrice {
                input_per_million: 15.0,
                output_per_million: 75.0,
            },
        );

        prices.insert(
            "claude-sonnet".to_string(),
            ModelPrice {
                input_per_million: 3.0,
                output_per_million: 15.0,
            },
        );

        prices.insert(
            "claude-haiku".to_string(),
            ModelPrice {
                input_per_million: 0.25,
                output_per_million: 1.25,
            },
        );

        prices.insert(
            "gpt-4o".to_string(),
            ModelPrice {
                input_per_million: 2.50,
                output_per_million: 10.0,
            },
        );

        prices.insert(
            "gpt-4o-mini".to_string(),
            ModelPrice {
                input_per_million: 0.15,
                output_per_million: 0.60,
            },
        );

        // Ollama (local) models are free.
        prices.insert(
            "ollama/".to_string(),
            ModelPrice {
                input_per_million: 0.0,
                output_per_million: 0.0,
            },
        );

        prices
    }

    /// Look up the price for a model, using prefix matching and a default fallback.
    fn get_price(&self, model: &str) -> &ModelPrice {
        // Exact match first.
        if let Some(price) = self.model_prices.get(model) {
            return price;
        }

        // Prefix match (e.g. "claude-sonnet" matches "claude-sonnet-4-20250514").
        for (key, price) in &self.model_prices {
            if model.starts_with(key) {
                return price;
            }
        }

        // Default fallback pricing.
        // We use a static leak-free approach with a const reference.
        static DEFAULT_PRICE: ModelPrice = ModelPrice {
            input_per_million: 1.0,
            output_per_million: 3.0,
        };
        &DEFAULT_PRICE
    }

    /// Calculate the cost for a given model and token counts.
    pub fn estimate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let price = self.get_price(model);
        let input_cost = (input_tokens as f64 / 1_000_000.0) * price.input_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * price.output_per_million;
        input_cost + output_cost
    }

    /// Record usage for a fighter, calculating cost automatically.
    #[instrument(skip(self), fields(%fighter_id, %model, input_tokens, output_tokens))]
    pub async fn record_usage(
        &self,
        fighter_id: &FighterId,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> PunchResult<f64> {
        let cost = self.estimate_cost(model, input_tokens, output_tokens);

        self.memory
            .record_usage(fighter_id, model, input_tokens, output_tokens, cost)
            .await?;

        debug!(cost_usd = cost, "usage recorded with cost");
        Ok(cost)
    }

    /// Get total spend for a specific fighter over a time period.
    pub async fn get_spend(&self, fighter_id: &FighterId, period: SpendPeriod) -> PunchResult<f64> {
        let since = Utc::now() - period.to_duration();
        let summary = self.memory.get_usage_summary(fighter_id, since).await?;
        Ok(summary.total_cost_usd)
    }

    /// Get total spend across all fighters over a time period.
    pub async fn get_total_spend(&self, period: SpendPeriod) -> PunchResult<f64> {
        let since = Utc::now() - period.to_duration();
        let summary = self.memory.get_total_usage_summary(since).await?;
        Ok(summary.total_cost_usd)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_claude_sonnet() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(memory);

        // claude-sonnet: $3/M in, $15/M out
        let cost = engine.estimate_cost("claude-sonnet-4-20250514", 1_000_000, 1_000_000);
        assert!((cost - 18.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_gpt4o_mini() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(memory);

        // gpt-4o-mini: $0.15/M in, $0.60/M out
        let cost = engine.estimate_cost("gpt-4o-mini", 1_000_000, 1_000_000);
        assert!((cost - 0.75).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_ollama_free() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(memory);

        let cost = engine.estimate_cost("ollama/llama3", 1_000_000, 1_000_000);
        assert!((cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_unknown_model_uses_fallback() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(memory);

        // Default fallback: $1/M in, $3/M out
        let cost = engine.estimate_cost("some-unknown-model", 1_000_000, 1_000_000);
        assert!((cost - 4.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_small_usage() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(memory);

        // 1000 input tokens, 500 output tokens with claude-sonnet
        let cost = engine.estimate_cost("claude-sonnet-4-20250514", 1000, 500);
        let expected = (1000.0 / 1_000_000.0) * 3.0 + (500.0 / 1_000_000.0) * 15.0;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[tokio::test]
    async fn record_and_query_usage() {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let engine = MeteringEngine::new(Arc::clone(&memory));

        let fighter_id = FighterId::new();

        // Save fighter first (FK constraint).
        use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};
        let manifest = FighterManifest {
            name: "metering-test".into(),
            description: "test".into(),
            model: ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".into(),
                api_key_env: None,
                base_url: None,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            system_prompt: "test".into(),
            capabilities: Vec::new(),
            weight_class: WeightClass::Featherweight,
        };
        memory
            .save_fighter(&fighter_id, &manifest, FighterStatus::Idle)
            .await
            .unwrap();

        let cost = engine
            .record_usage(&fighter_id, "claude-sonnet-4-20250514", 5000, 2000)
            .await
            .unwrap();

        // claude-sonnet: $3/M in, $15/M out
        let expected = (5000.0 / 1_000_000.0) * 3.0 + (2000.0 / 1_000_000.0) * 15.0;
        assert!((cost - expected).abs() < 1e-12);

        // Query the spend.
        let spend = engine
            .get_spend(&fighter_id, SpendPeriod::Hour)
            .await
            .unwrap();
        assert!((spend - expected).abs() < 1e-9);
    }

    #[test]
    fn spend_period_display() {
        assert_eq!(SpendPeriod::Hour.to_string(), "hour");
        assert_eq!(SpendPeriod::Day.to_string(), "day");
        assert_eq!(SpendPeriod::Month.to_string(), "month");
    }
}
