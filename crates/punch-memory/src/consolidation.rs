//! # Memory Consolidation
//!
//! Training memory consolidation for the Punch Agent Combat System.
//!
//! Over time, fighters accumulate vast amounts of memory from their bouts and
//! interactions. Left unchecked, this leads to bloated recall and slower
//! decision-making — like a fighter carrying too much muscle mass for their
//! weight class.
//!
//! The [`MemoryConsolidator`] periodically merges, decays, and prunes old
//! memories, keeping the fighter's muscle memory sharp and efficient. Think of
//! it as the recovery phase between bouts: the body (memory store) sheds what
//! it doesn't need and strengthens what matters.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use punch_types::{FighterId, PunchError, PunchResult};

use crate::MemorySubstrate;
use crate::memories::MemoryEntry;

/// Configuration that governs how aggressively a fighter's memories are
/// consolidated — the training regimen for muscle memory retention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationConfig {
    /// Maximum entries before triggering consolidation.
    pub max_memories_per_fighter: usize,
    /// Trigger consolidation when this count is exceeded.
    pub consolidation_threshold: usize,
    /// Minimum confidence to keep a memory alive.
    pub min_confidence: f64,
    /// Daily decay rate applied to confidence scores (0.0–1.0).
    pub decay_rate: f64,
    /// Similarity threshold for merging related memories (0.0–1.0).
    pub merge_similarity_threshold: f64,
    /// Maximum age in days before a memory becomes a candidate for pruning.
    pub max_age_days: u64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            max_memories_per_fighter: 1000,
            consolidation_threshold: 800,
            min_confidence: 0.3,
            decay_rate: 0.01,
            merge_similarity_threshold: 0.8,
            max_age_days: 90,
        }
    }
}

/// Results from a consolidation round — the post-training report card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationResult {
    /// Number of memories before consolidation.
    pub memories_before: usize,
    /// Number of memories after consolidation.
    pub memories_after: usize,
    /// Number of memories merged with similar entries.
    pub merged: usize,
    /// Number of memories pruned (removed entirely).
    pub pruned: usize,
    /// Number of memories whose confidence was decayed.
    pub decayed: usize,
    /// How long the consolidation took, in milliseconds.
    pub duration_ms: u64,
}

/// The memory consolidation engine — responsible for keeping a fighter's
/// memory store lean and battle-ready.
///
/// Like a seasoned corner crew between rounds, the consolidator trims the fat,
/// reinforces core muscle memory, and ensures the fighter enters the next bout
/// at peak mental sharpness.
#[derive(Debug, Clone)]
pub struct MemoryConsolidator {
    /// The consolidation training regimen.
    pub config: ConsolidationConfig,
}

impl MemoryConsolidator {
    /// Create a new consolidator with the given configuration.
    pub fn new(config: ConsolidationConfig) -> Self {
        Self { config }
    }

    /// Create a consolidator with sensible default settings — a balanced
    /// training regimen suitable for most fighters.
    pub fn with_defaults() -> Self {
        Self {
            config: ConsolidationConfig::default(),
        }
    }

    /// Run full memory consolidation for a fighter — the complete recovery
    /// session between bouts.
    ///
    /// Steps:
    /// 1. Apply confidence decay based on memory age
    /// 2. Prune memories below the minimum confidence threshold
    /// 3. Merge similar memories (by key similarity)
    /// 4. If still over max capacity, prune lowest-confidence oldest memories
    pub async fn consolidate(
        &self,
        memory: &MemorySubstrate,
        fighter_id: &FighterId,
    ) -> PunchResult<ConsolidationResult> {
        let start = std::time::Instant::now();
        let mut merged_count = 0usize;
        let mut pruned_count = 0usize;
        let mut decayed_count = 0usize;

        // Fetch all memories for this fighter.
        let all_memories = self.fetch_all_memories(memory, fighter_id).await?;
        let memories_before = all_memories.len();

        info!(
            fighter_id = %fighter_id,
            memory_count = memories_before,
            "beginning memory consolidation — entering recovery phase"
        );

        // Step 1: Apply confidence decay based on age.
        let now = Utc::now();
        for entry in &all_memories {
            let age_days = (now - entry.created_at).num_seconds() as f64 / 86400.0;
            if age_days > 0.0 {
                let new_confidence = self.apply_decay(entry.confidence, age_days);
                if (new_confidence - entry.confidence).abs() > f64::EPSILON {
                    memory
                        .store_memory(fighter_id, &entry.key, &entry.value, new_confidence)
                        .await?;
                    decayed_count += 1;
                }
            }
        }

        // Step 2: Prune memories below min_confidence threshold.
        let after_decay = self.fetch_all_memories(memory, fighter_id).await?;
        for entry in &after_decay {
            if entry.confidence < self.config.min_confidence {
                memory.delete_memory(fighter_id, &entry.key).await?;
                pruned_count += 1;
            }
        }

        // Step 3: Merge similar memories by key similarity.
        let after_prune = self.fetch_all_memories(memory, fighter_id).await?;
        let mut consumed: Vec<bool> = vec![false; after_prune.len()];

        for i in 0..after_prune.len() {
            if consumed[i] {
                continue;
            }
            let mut group: Vec<(&str, f64)> =
                vec![(after_prune[i].value.as_str(), after_prune[i].confidence)];
            let mut merge_keys: Vec<usize> = Vec::new();

            for j in (i + 1)..after_prune.len() {
                if consumed[j] {
                    continue;
                }
                if Self::keys_are_similar(&after_prune[i].key, &after_prune[j].key) {
                    group.push((after_prune[j].value.as_str(), after_prune[j].confidence));
                    merge_keys.push(j);
                }
            }

            if !merge_keys.is_empty() {
                // Merge: pick best value and average confidence.
                let (merged_value, merged_confidence) = Self::merge_values(&group);

                // Delete the consumed entries.
                for &idx in &merge_keys {
                    memory
                        .delete_memory(fighter_id, &after_prune[idx].key)
                        .await?;
                    consumed[idx] = true;
                    merged_count += 1;
                }

                // Update the surviving entry with merged data.
                memory
                    .store_memory(
                        fighter_id,
                        &after_prune[i].key,
                        &merged_value,
                        merged_confidence,
                    )
                    .await?;
            }
        }

        // Step 4: If still over max, prune lowest-confidence oldest memories.
        let mut current = self.fetch_all_memories(memory, fighter_id).await?;
        if current.len() > self.config.max_memories_per_fighter {
            // Sort by confidence ascending, then by age descending (oldest first).
            current.sort_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            });

            let excess = current.len() - self.config.max_memories_per_fighter;
            for entry in current.iter().take(excess) {
                memory.delete_memory(fighter_id, &entry.key).await?;
                pruned_count += 1;
            }
        }

        let memories_after = self.fetch_all_memories(memory, fighter_id).await?.len();

        let duration_ms = start.elapsed().as_millis() as u64;

        let result = ConsolidationResult {
            memories_before,
            memories_after,
            merged: merged_count,
            pruned: pruned_count,
            decayed: decayed_count,
            duration_ms,
        };

        info!(
            fighter_id = %fighter_id,
            before = memories_before,
            after = memories_after,
            merged = merged_count,
            pruned = pruned_count,
            decayed = decayed_count,
            duration_ms = duration_ms,
            "memory consolidation complete — fighter is battle-ready"
        );

        Ok(result)
    }

    /// Compute decayed confidence based on age.
    ///
    /// Muscle memory fades without practice — confidence erodes over time
    /// at the configured decay rate: `confidence * (1.0 - decay_rate) ^ age_days`.
    pub fn apply_decay(&self, confidence: f64, age_days: f64) -> f64 {
        let decayed = confidence * (1.0 - self.config.decay_rate).powf(age_days);
        // Confidence can never drop below zero.
        decayed.max(0.0)
    }

    /// Check whether a fighter's memory store needs consolidation — is the
    /// fighter carrying too much weight for their class?
    pub fn should_consolidate(&self, memory_count: usize) -> bool {
        memory_count > self.config.consolidation_threshold
    }

    /// Check if two memory keys are similar enough to merge.
    ///
    /// Uses a normalized edit distance (Levenshtein-like). Two keys are
    /// considered similar if their normalized similarity score exceeds the
    /// configured threshold — like recognizing two punches as variations
    /// of the same combo.
    pub fn keys_are_similar(a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        let similarity = normalized_similarity(a, b);
        // Use a fixed threshold of 0.8 for the static method.
        // For instance-based checks, use `keys_are_similar_with_threshold`.
        similarity >= 0.8
    }

    /// Merge multiple memory values into a single consolidated entry.
    ///
    /// Like combining footage from multiple training sessions: the best
    /// performance (highest confidence value) is kept, and the overall
    /// confidence is averaged across all observations.
    pub fn merge_values(values: &[(&str, f64)]) -> (String, f64) {
        if values.is_empty() {
            return (String::new(), 0.0);
        }

        // Pick the value with the highest confidence.
        let best = values
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(v, _)| v.to_string())
            .unwrap_or_default();

        // Average the confidences.
        let avg_confidence = values.iter().map(|(_, c)| c).sum::<f64>() / values.len() as f64;

        (best, avg_confidence)
    }

    /// Fetch all memories for a fighter directly from the database.
    ///
    /// This bypasses the query-based `recall_memories` to get a complete
    /// inventory of the fighter's memory store.
    async fn fetch_all_memories(
        &self,
        memory: &MemorySubstrate,
        fighter_id: &FighterId,
    ) -> PunchResult<Vec<MemoryEntry>> {
        let fighter_str = fighter_id.to_string();
        let conn = memory.conn().await;

        let mut stmt = conn
            .prepare(
                "SELECT key, value, confidence, created_at, accessed_at
                 FROM memories
                 WHERE fighter_id = ?1
                 ORDER BY confidence DESC",
            )
            .map_err(|e| PunchError::Memory(format!("failed to fetch all memories: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fighter_str], |row| {
                let key: String = row.get(0)?;
                let value: String = row.get(1)?;
                let confidence: f64 = row.get(2)?;
                let created_at: String = row.get(3)?;
                let accessed_at: String = row.get(4)?;
                Ok((key, value, confidence, created_at, accessed_at))
            })
            .map_err(|e| PunchError::Memory(format!("failed to fetch all memories: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            let (key, value, confidence, created_at, accessed_at) =
                row.map_err(|e| PunchError::Memory(format!("failed to read memory row: {e}")))?;

            let created_at = parse_ts(&created_at)?;
            let accessed_at = parse_ts(&accessed_at)?;

            entries.push(MemoryEntry {
                key,
                value,
                confidence,
                created_at,
                accessed_at,
            });
        }

        debug!(
            fighter_id = %fighter_id,
            count = entries.len(),
            "fetched all memories for consolidation"
        );

        Ok(entries)
    }
}

/// Compute the Levenshtein edit distance between two strings.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Use a single-row optimization for the DP table.
    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row: Vec<usize> = vec![0; b_len + 1];

    for i in 1..=a_len {
        curr_row[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr_row[j] = (prev_row[j] + 1)
                .min(curr_row[j - 1] + 1)
                .min(prev_row[j - 1] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[b_len]
}

/// Compute normalized similarity between two strings (0.0 = completely different,
/// 1.0 = identical).
fn normalized_similarity(a: &str, b: &str) -> f64 {
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

/// Parse a timestamp string into a `DateTime<Utc>`.
fn parse_ts(s: &str) -> PunchResult<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").map(|ndt| ndt.and_utc())
        })
        .map_err(|e| PunchError::Memory(format!("invalid timestamp '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "Consolidation Fighter".into(),
            description: "memory consolidation test".into(),
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
            tenant_id: None,
        }
    }

    fn default_consolidator() -> MemoryConsolidator {
        MemoryConsolidator::with_defaults()
    }

    // --- Confidence decay tests ---

    #[test]
    fn test_decay_zero_days_no_change() {
        let c = default_consolidator();
        let result = c.apply_decay(0.9, 0.0);
        assert!(
            (result - 0.9).abs() < f64::EPSILON,
            "0 days should produce no decay"
        );
    }

    #[test]
    fn test_decay_30_days_significant() {
        let c = default_consolidator();
        let result = c.apply_decay(1.0, 30.0);
        // With 0.01 daily decay: 1.0 * 0.99^30 ≈ 0.7397
        assert!(
            result < 0.75,
            "30 days should produce significant decay, got {result}"
        );
        assert!(
            result > 0.70,
            "30 days decay should not be too aggressive, got {result}"
        );
    }

    #[test]
    fn test_decay_does_not_go_below_zero() {
        let c = default_consolidator();
        // Even with extreme age, confidence stays >= 0.
        let result = c.apply_decay(0.01, 100_000.0);
        assert!(result >= 0.0, "decayed confidence must never be negative");
    }

    // --- should_consolidate tests ---

    #[test]
    fn test_should_consolidate_triggers_at_threshold() {
        let c = default_consolidator();
        // Default threshold is 800.
        assert!(c.should_consolidate(801), "should trigger above threshold");
        assert!(
            c.should_consolidate(1000),
            "should trigger well above threshold"
        );
    }

    #[test]
    fn test_should_consolidate_false_below_threshold() {
        let c = default_consolidator();
        assert!(
            !c.should_consolidate(800),
            "should not trigger at exactly threshold"
        );
        assert!(
            !c.should_consolidate(500),
            "should not trigger below threshold"
        );
        assert!(
            !c.should_consolidate(0),
            "should not trigger with no memories"
        );
    }

    // --- keys_are_similar tests ---

    #[test]
    fn test_keys_identical_match() {
        assert!(
            MemoryConsolidator::keys_are_similar("user_preference", "user_preference"),
            "identical keys must match"
        );
    }

    #[test]
    fn test_keys_very_different_no_match() {
        assert!(
            !MemoryConsolidator::keys_are_similar("user_preference", "system_config_debug_level"),
            "very different keys must not match"
        );
    }

    #[test]
    fn test_keys_similar_match() {
        // "user_preference" vs "user_preferences" — one character difference.
        assert!(
            MemoryConsolidator::keys_are_similar("user_preference", "user_preferences"),
            "similar keys (singular vs plural) should match"
        );
    }

    // --- merge_values tests ---

    #[test]
    fn test_merge_values_picks_highest_confidence() {
        let values = vec![("low_val", 0.3), ("high_val", 0.9), ("mid_val", 0.6)];
        let (value, _) = MemoryConsolidator::merge_values(&values);
        assert_eq!(
            value, "high_val",
            "should pick the value with highest confidence"
        );
    }

    #[test]
    fn test_merge_values_averages_confidences() {
        let values = vec![("a", 0.3), ("b", 0.9), ("c", 0.6)];
        let (_, avg) = MemoryConsolidator::merge_values(&values);
        let expected = (0.3 + 0.9 + 0.6) / 3.0;
        assert!(
            (avg - expected).abs() < f64::EPSILON,
            "should average confidences: expected {expected}, got {avg}"
        );
    }

    // --- Full consolidation integration tests ---

    #[tokio::test]
    async fn test_full_consolidation_reduces_count() {
        let substrate = MemorySubstrate::in_memory().expect("in-memory substrate");
        let fid = FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .expect("save fighter");

        // Store many memories — some with low confidence that should be pruned.
        for i in 0..20 {
            let confidence = if i < 5 { 0.1 } else { 0.8 };
            substrate
                .store_memory(
                    &fid,
                    &format!("memory_{i}"),
                    &format!("value_{i}"),
                    confidence,
                )
                .await
                .expect("store memory");
        }

        let consolidator = MemoryConsolidator::new(ConsolidationConfig {
            max_memories_per_fighter: 100,
            consolidation_threshold: 10,
            min_confidence: 0.3,
            decay_rate: 0.0, // No decay for this test.
            merge_similarity_threshold: 0.8,
            max_age_days: 90,
        });

        let result = consolidator
            .consolidate(&substrate, &fid)
            .await
            .expect("consolidation");

        assert_eq!(result.memories_before, 20);
        assert!(
            result.memories_after < result.memories_before,
            "consolidation should reduce memory count"
        );
        assert!(
            result.pruned > 0,
            "should have pruned some low-confidence memories"
        );
    }

    #[tokio::test]
    async fn test_pruning_removes_low_confidence() {
        let substrate = MemorySubstrate::in_memory().expect("in-memory substrate");
        let fid = FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .expect("save fighter");

        // Store memories: some above threshold, some below.
        substrate
            .store_memory(&fid, "strong_memory", "important", 0.9)
            .await
            .expect("store");
        substrate
            .store_memory(&fid, "weak_memory", "forgettable", 0.1)
            .await
            .expect("store");
        substrate
            .store_memory(&fid, "medium_memory", "moderate", 0.5)
            .await
            .expect("store");

        let consolidator = MemoryConsolidator::new(ConsolidationConfig {
            min_confidence: 0.3,
            decay_rate: 0.0,
            ..ConsolidationConfig::default()
        });

        let result = consolidator
            .consolidate(&substrate, &fid)
            .await
            .expect("consolidation");

        // The weak memory (0.1) should have been pruned.
        assert!(result.pruned >= 1, "should prune at least the weak memory");

        // Verify the weak memory is gone.
        let remaining = substrate
            .recall_memories(&fid, "weak_memory", 10)
            .await
            .expect("recall");
        assert!(remaining.is_empty(), "weak memory should be pruned");

        // Verify the strong memory survives.
        let strong = substrate
            .recall_memories(&fid, "strong_memory", 10)
            .await
            .expect("recall");
        assert_eq!(
            strong.len(),
            1,
            "strong memory should survive consolidation"
        );
    }

    #[test]
    fn test_config_defaults_are_sensible() {
        let config = ConsolidationConfig::default();
        assert_eq!(config.max_memories_per_fighter, 1000);
        assert_eq!(config.consolidation_threshold, 800);
        assert!((config.min_confidence - 0.3).abs() < f64::EPSILON);
        assert!((config.decay_rate - 0.01).abs() < f64::EPSILON);
        assert!((config.merge_similarity_threshold - 0.8).abs() < f64::EPSILON);
        assert_eq!(config.max_age_days, 90);
        // Threshold must be less than max to make sense.
        assert!(
            config.consolidation_threshold < config.max_memories_per_fighter,
            "threshold should be below max"
        );
    }

    // --- Edit distance tests ---

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein_distance("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_empty() {
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
    }

    #[test]
    fn test_levenshtein_one_edit() {
        assert_eq!(levenshtein_distance("cat", "cats"), 1);
        assert_eq!(levenshtein_distance("cat", "car"), 1);
    }

    #[test]
    fn test_normalized_similarity_identical() {
        let sim = normalized_similarity("test", "test");
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_normalized_similarity_completely_different() {
        let sim = normalized_similarity("abc", "xyz");
        assert!(
            sim < 0.5,
            "completely different strings should have low similarity"
        );
    }

    #[test]
    fn test_levenshtein_both_empty() {
        assert_eq!(levenshtein_distance("", ""), 0);
    }

    #[test]
    fn test_levenshtein_substitutions() {
        assert_eq!(levenshtein_distance("abc", "xyz"), 3);
    }

    #[test]
    fn test_normalized_similarity_both_empty() {
        let sim = normalized_similarity("", "");
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_normalized_similarity_one_empty() {
        let sim = normalized_similarity("hello", "");
        assert!((sim - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_merge_values_empty() {
        let (value, confidence) = MemoryConsolidator::merge_values(&[]);
        assert!(value.is_empty());
        assert!((confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_merge_values_single() {
        let (value, confidence) = MemoryConsolidator::merge_values(&[("only", 0.5)]);
        assert_eq!(value, "only");
        assert!((confidence - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decay_custom_rate() {
        let config = ConsolidationConfig {
            decay_rate: 0.1,
            ..ConsolidationConfig::default()
        };
        let c = MemoryConsolidator::new(config);
        let result = c.apply_decay(1.0, 10.0);
        // 1.0 * 0.9^10 ≈ 0.3486
        assert!(result < 0.4);
        assert!(result > 0.3);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = ConsolidationConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: ConsolidationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.max_memories_per_fighter,
            config.max_memories_per_fighter
        );
        assert_eq!(restored.max_age_days, config.max_age_days);
    }

    #[test]
    fn test_result_serde_roundtrip() {
        let result = ConsolidationResult {
            memories_before: 100,
            memories_after: 80,
            merged: 5,
            pruned: 15,
            decayed: 90,
            duration_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: ConsolidationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.memories_before, 100);
        assert_eq!(restored.pruned, 15);
    }

    #[test]
    fn test_keys_similar_empty_strings() {
        assert!(MemoryConsolidator::keys_are_similar("", ""));
    }
}
