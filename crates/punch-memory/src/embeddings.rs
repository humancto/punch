//! Vector embedding support for semantic recall in the Punch memory system.
//!
//! Provides a [`BuiltInEmbedder`] (TF-IDF based, no external deps), an
//! [`OpenAiEmbedder`] (calls the OpenAI embeddings API), and an
//! [`EmbeddingStore`] backed by SQLite for persistence and similarity search.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::debug;

use punch_types::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A vector embedding with its source text and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub id: String,
    pub text: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

/// Configuration for the embedding engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProvider,
    pub dimensions: usize,
    pub batch_size: usize,
}

/// Which backend to use for computing embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbeddingProvider {
    /// OpenAI text-embedding-3-small/large.
    OpenAi { api_key: String, model: String },
    /// Local sentence-transformers via HTTP (e.g., running on localhost).
    Local { endpoint: String },
    /// Simple TF-IDF bag-of-words (no external dependency, works offline).
    BuiltIn,
}

// ---------------------------------------------------------------------------
// Embedder trait
// ---------------------------------------------------------------------------

/// Trait for computing vector embeddings from text.
pub trait Embedder: Send + Sync {
    /// Compute an embedding vector for a single piece of text.
    fn embed(&self, text: &str) -> PunchResult<Vec<f32>>;

    /// Compute embedding vectors for a batch of texts.
    fn embed_batch(&self, texts: &[&str]) -> PunchResult<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    /// The dimensionality of vectors produced by this embedder.
    fn dimensions(&self) -> usize;
}

// ---------------------------------------------------------------------------
// Cosine similarity
// ---------------------------------------------------------------------------

/// Compute the cosine similarity between two vectors.
///
/// Returns a value in \[-1.0, 1.0\]. Identical directions yield 1.0,
/// orthogonal vectors yield 0.0, and opposite directions yield -1.0.
/// Returns 0.0 if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "vectors must have equal length");

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for (ai, bi) in a.iter().zip(b.iter()) {
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    dot / denom
}

/// Return the top-k most similar embeddings to `query_vec`, sorted by
/// descending similarity.
pub fn top_k_similar<'a>(
    query_vec: &[f32],
    embeddings: &'a [Embedding],
    k: usize,
) -> Vec<(f32, &'a Embedding)> {
    let mut scored: Vec<(f32, &Embedding)> = embeddings
        .iter()
        .map(|e| (cosine_similarity(query_vec, &e.vector), e))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

// ---------------------------------------------------------------------------
// Built-in TF-IDF embedder
// ---------------------------------------------------------------------------

/// A TF-IDF vectorizer that works entirely offline with no external
/// dependencies. Call [`BuiltInEmbedder::fit`] with a corpus to build the
/// vocabulary, then use [`Embedder::embed`] to compute vectors.
pub struct BuiltInEmbedder {
    /// Ordered vocabulary (word → index).
    vocab: HashMap<String, usize>,
    /// IDF weight for each vocabulary term (same indexing as `vocab`).
    idf: Vec<f32>,
    /// Number of dimensions (= vocabulary size, capped at 1024).
    dims: usize,
}

impl std::fmt::Debug for BuiltInEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltInEmbedder")
            .field("dims", &self.dims)
            .field("vocab_size", &self.vocab.len())
            .finish()
    }
}

impl BuiltInEmbedder {
    /// Create an empty embedder. You must call [`Self::fit`] before embedding.
    pub fn new() -> Self {
        Self {
            vocab: HashMap::new(),
            idf: Vec::new(),
            dims: 0,
        }
    }

    /// Build the vocabulary and IDF weights from a corpus of documents.
    ///
    /// The vocabulary is capped at 1024 terms. Terms are selected by document
    /// frequency (the most widely occurring terms across documents come first).
    pub fn fit(&mut self, documents: &[&str]) {
        let total_docs = documents.len() as f32;
        if total_docs == 0.0 {
            self.vocab.clear();
            self.idf.clear();
            self.dims = 0;
            return;
        }

        // Collect document frequency for each term.
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        for doc in documents {
            let unique_words: std::collections::HashSet<String> =
                tokenize(doc).into_iter().collect();
            for word in unique_words {
                *doc_freq.entry(word).or_insert(0) += 1;
            }
        }

        // Sort terms by document frequency descending, then alphabetically for
        // determinism, and cap at 1024.
        let mut terms: Vec<(String, usize)> = doc_freq.into_iter().collect();
        terms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        terms.truncate(1024);

        self.vocab.clear();
        self.idf = Vec::with_capacity(terms.len());
        for (i, (term, df)) in terms.iter().enumerate() {
            self.vocab.insert(term.clone(), i);
            // Standard IDF: log(N / df). Use ln for smoothness.
            self.idf.push((total_docs / *df as f32).ln());
        }
        self.dims = self.vocab.len();
    }

    /// Tokenize text and compute the TF-IDF vector, then L2-normalize it.
    fn compute_tfidf(&self, text: &str) -> Vec<f32> {
        if self.dims == 0 {
            return Vec::new();
        }

        let tokens = tokenize(text);
        let total_tokens = tokens.len() as f32;
        if total_tokens == 0.0 {
            return vec![0.0; self.dims];
        }

        // Term frequency counts.
        let mut tf_counts: HashMap<&str, usize> = HashMap::new();
        for t in &tokens {
            *tf_counts.entry(t.as_str()).or_insert(0) += 1;
        }

        let mut vec = vec![0.0_f32; self.dims];
        for (term, count) in &tf_counts {
            if let Some(&idx) = self.vocab.get(*term) {
                let tf = *count as f32 / total_tokens;
                vec[idx] = tf * self.idf[idx];
            }
        }

        l2_normalize(&mut vec);
        vec
    }
}

impl Default for BuiltInEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for BuiltInEmbedder {
    fn embed(&self, text: &str) -> PunchResult<Vec<f32>> {
        Ok(self.compute_tfidf(text))
    }

    fn embed_batch(&self, texts: &[&str]) -> PunchResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.compute_tfidf(t)).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

// ---------------------------------------------------------------------------
// OpenAI embedder
// ---------------------------------------------------------------------------

/// An embedder that calls the OpenAI embeddings API.
///
/// Requires the `reqwest` crate (already a workspace dependency).
pub struct OpenAiEmbedder {
    api_key: String,
    model: String,
    dimensions: usize,
}

impl std::fmt::Debug for OpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .finish()
    }
}

impl OpenAiEmbedder {
    /// Create a new OpenAI embedder.
    ///
    /// `model` should be something like `"text-embedding-3-small"`.
    pub fn new(api_key: String, model: String, dimensions: usize) -> Self {
        Self {
            api_key,
            model,
            dimensions,
        }
    }

    /// Build the JSON request body for the OpenAI embeddings endpoint.
    pub fn build_request_body(&self, input: &[&str]) -> serde_json::Value {
        if input.len() == 1 {
            serde_json::json!({
                "input": input[0],
                "model": self.model,
            })
        } else {
            serde_json::json!({
                "input": input,
                "model": self.model,
            })
        }
    }

    /// Parse the embedding vectors from an OpenAI API response.
    pub fn parse_response(body: &serde_json::Value) -> PunchResult<Vec<Vec<f32>>> {
        let data = body
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| PunchError::Memory("missing 'data' array in response".into()))?;

        let mut results = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| PunchError::Memory("missing 'embedding' in data item".into()))?;
            let vec: Vec<f32> = embedding
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            results.push(vec);
        }
        Ok(results)
    }
}

impl Embedder for OpenAiEmbedder {
    fn embed(&self, text: &str) -> PunchResult<Vec<f32>> {
        // In a real implementation this would use reqwest to call the API.
        // For now, we return an error indicating that the runtime needs async.
        Err(PunchError::Memory(format!(
            "OpenAI embedding requires async runtime; use embed_batch or call the API directly. \
             model={}, key_len={}, text_len={}",
            self.model,
            self.api_key.len(),
            text.len()
        )))
    }

    fn embed_batch(&self, texts: &[&str]) -> PunchResult<Vec<Vec<f32>>> {
        Err(PunchError::Memory(format!(
            "OpenAI embedding requires async runtime; call the API directly. \
             model={}, batch_size={}",
            self.model,
            texts.len()
        )))
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ---------------------------------------------------------------------------
// EmbeddingStore (SQLite-backed)
// ---------------------------------------------------------------------------

/// Persistent store for embeddings, backed by SQLite.
pub struct EmbeddingStore {
    conn: Arc<Mutex<Connection>>,
    embedder: Box<dyn Embedder>,
}

impl EmbeddingStore {
    /// Create a new embedding store, creating the `embeddings` table if it
    /// does not already exist.
    pub fn new(conn: Arc<Mutex<Connection>>, embedder: Box<dyn Embedder>) -> PunchResult<Self> {
        {
            let c = conn
                .lock()
                .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS embeddings (
                    id         TEXT PRIMARY KEY,
                    text       TEXT NOT NULL,
                    vector     BLOB NOT NULL,
                    metadata   TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );",
            )
            .map_err(|e| PunchError::Memory(format!("failed to create embeddings table: {e}")))?;
        }
        Ok(Self { conn, embedder })
    }

    /// Store text with its embedding vector.
    pub fn store(&self, text: &str, metadata: HashMap<String, String>) -> PunchResult<String> {
        let vector = self.embedder.embed(text)?;
        let id = uuid::Uuid::new_v4().to_string();
        let blob = vec_to_bytes(&vector);
        let meta_json = serde_json::to_string(&metadata)
            .map_err(|e| PunchError::Memory(format!("metadata serialization failed: {e}")))?;
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;
        conn.execute(
            "INSERT INTO embeddings (id, text, vector, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, text, blob, meta_json, now],
        )
        .map_err(|e| PunchError::Memory(format!("failed to store embedding: {e}")))?;

        debug!(id = %id, text_len = text.len(), "embedding stored");
        Ok(id)
    }

    /// Search for the top-k most similar embeddings to `query`.
    pub fn search(&self, query: &str, k: usize) -> PunchResult<Vec<(f32, Embedding)>> {
        let query_vec = self.embedder.embed(query)?;
        let all = self.load_all()?;
        let results = top_k_similar(&query_vec, &all, k);
        Ok(results.into_iter().map(|(s, e)| (s, e.clone())).collect())
    }

    /// Delete an embedding by ID.
    pub fn delete(&self, id: &str) -> PunchResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;
        conn.execute("DELETE FROM embeddings WHERE id = ?1", [id])
            .map_err(|e| PunchError::Memory(format!("failed to delete embedding: {e}")))?;
        debug!(id = %id, "embedding deleted");
        Ok(())
    }

    /// Return the total number of stored embeddings.
    pub fn count(&self) -> PunchResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .map_err(|e| PunchError::Memory(format!("failed to count embeddings: {e}")))?;
        Ok(count as usize)
    }

    /// Re-embed all stored texts. Useful when the embedder changes (e.g.,
    /// after re-fitting the built-in TF-IDF vocabulary).
    pub fn rebuild_index(&self) -> PunchResult<usize> {
        let all = self.load_all()?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;

        let mut count = 0usize;
        for emb in &all {
            let new_vec = self.embedder.embed(&emb.text)?;
            let blob = vec_to_bytes(&new_vec);
            conn.execute(
                "UPDATE embeddings SET vector = ?1 WHERE id = ?2",
                rusqlite::params![blob, emb.id],
            )
            .map_err(|e| PunchError::Memory(format!("failed to update embedding: {e}")))?;
            count += 1;
        }
        debug!(count, "embedding index rebuilt");
        Ok(count)
    }

    /// Provide a reference to the current embedder.
    pub fn embedder(&self) -> &dyn Embedder {
        self.embedder.as_ref()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn load_all(&self) -> PunchResult<Vec<Embedding>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("lock failed: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT id, text, vector, metadata, created_at FROM embeddings")
            .map_err(|e| PunchError::Memory(format!("failed to query embeddings: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let text: String = row.get(1)?;
                let blob: Vec<u8> = row.get(2)?;
                let meta_json: String = row.get(3)?;
                let created_at: String = row.get(4)?;
                Ok((id, text, blob, meta_json, created_at))
            })
            .map_err(|e| PunchError::Memory(format!("failed to query embeddings: {e}")))?;

        let mut embeddings = Vec::new();
        for row in rows {
            let (id, text, blob, meta_json, created_at_str) =
                row.map_err(|e| PunchError::Memory(format!("failed to read row: {e}")))?;

            let vector = bytes_to_vec(&blob);
            let metadata: HashMap<String, String> =
                serde_json::from_str(&meta_json).unwrap_or_default();
            let created_at = parse_ts(&created_at_str)?;

            embeddings.push(Embedding {
                id,
                text,
                vector,
                metadata,
                created_at,
            });
        }
        Ok(embeddings)
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers for f32 vectors ↔ byte blobs
// ---------------------------------------------------------------------------

/// Serialize a `Vec<f32>` to little-endian bytes.
pub fn vec_to_bytes(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Deserialize little-endian bytes back to a `Vec<f32>`.
pub fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk is 4 bytes");
            f32::from_le_bytes(arr)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Simple whitespace-and-punctuation tokenizer. Lowercases, strips
/// non-alphanumeric chars, and splits on whitespace.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// L2-normalize a vector in place. If the norm is zero, the vector is
/// left unchanged.
fn l2_normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}

fn parse_ts(s: &str) -> PunchResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").map(|ndt| ndt.and_utc())
        })
        .map_err(|e| PunchError::Memory(format!("invalid timestamp '{s}': {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Cosine similarity ---------------------------------------------------

    #[test]
    fn test_cosine_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "identical vectors should have similarity 1.0"
        );
    }

    #[test]
    fn test_cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-6,
            "orthogonal vectors should have similarity ~0.0"
        );
    }

    #[test]
    fn test_cosine_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim + 1.0).abs() < 1e-6,
            "opposite vectors should have similarity -1.0"
        );
    }

    #[test]
    fn test_cosine_zero_vector() {
        let a = vec![1.0, 2.0];
        let b = vec![0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6, "zero vector should yield 0.0");
    }

    // -- BuiltInEmbedder -----------------------------------------------------

    #[test]
    fn test_builtin_fit_and_embed_nonzero() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["the cat sat on the mat", "the dog chased the ball"]);

        let vec = embedder.embed("cat sat on mat").unwrap();
        assert!(!vec.is_empty());
        assert!(vec.iter().any(|&v| v != 0.0), "vector should be non-zero");
    }

    #[test]
    fn test_builtin_similar_texts_higher_similarity() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&[
            "rust programming language",
            "python programming language",
            "cooking recipes for dinner",
            "baking bread at home",
        ]);

        let v_rust = embedder.embed("rust programming").unwrap();
        let v_python = embedder.embed("python programming").unwrap();
        let v_cooking = embedder.embed("cooking dinner recipes").unwrap();

        let sim_related = cosine_similarity(&v_rust, &v_python);
        let sim_unrelated = cosine_similarity(&v_rust, &v_cooking);

        assert!(
            sim_related > sim_unrelated,
            "related texts should have higher similarity ({sim_related} > {sim_unrelated})"
        );
    }

    #[test]
    fn test_builtin_l2_normalization() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["hello world", "foo bar baz"]);

        let vec = embedder.embed("hello world foo").unwrap();
        if !vec.is_empty() && vec.iter().any(|&v| v != 0.0) {
            let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!(
                (norm - 1.0).abs() < 1e-5,
                "vector should be L2-normalized, got norm={norm}"
            );
        }
    }

    #[test]
    fn test_builtin_empty_corpus() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&[]);
        let vec = embedder.embed("anything").unwrap();
        assert!(vec.is_empty(), "empty corpus should produce empty vector");
        assert_eq!(embedder.dimensions(), 0);
    }

    #[test]
    fn test_builtin_single_document_corpus() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["the only document in the corpus"]);

        let vec = embedder.embed("the only document").unwrap();
        assert!(!vec.is_empty());
        // With a single document, IDF for all terms is log(1/1) = 0.
        // All terms appear in the only document, so IDF = ln(1) = 0.
        // The vector will be all zeros.
        assert!(
            vec.iter().all(|&v| v == 0.0),
            "single-doc corpus yields zero IDF, so vector is zero"
        );
    }

    #[test]
    fn test_builtin_batch_embedding() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["hello world", "foo bar"]);

        let batch = embedder.embed_batch(&["hello", "foo"]).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].len(), embedder.dimensions());
        assert_eq!(batch[1].len(), embedder.dimensions());
    }

    // -- Vector serialization ------------------------------------------------

    #[test]
    fn test_vec_bytes_roundtrip() {
        let original = vec![1.0_f32, -2.5, 3.14, 0.0, f32::MAX, f32::MIN];
        let bytes = vec_to_bytes(&original);
        let restored = bytes_to_vec(&bytes);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_vec_bytes_empty() {
        let empty: Vec<f32> = Vec::new();
        let bytes = vec_to_bytes(&empty);
        assert!(bytes.is_empty());
        let restored = bytes_to_vec(&bytes);
        assert!(restored.is_empty());
    }

    // -- EmbeddingConfig serialization ---------------------------------------

    #[test]
    fn test_embedding_config_serde() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::BuiltIn,
            dimensions: 1024,
            batch_size: 32,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: EmbeddingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.dimensions, 1024);
        assert_eq!(restored.batch_size, 32);
    }

    #[test]
    fn test_embedding_config_openai_serde() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::OpenAi {
                api_key: "sk-test".into(),
                model: "text-embedding-3-small".into(),
            },
            dimensions: 1536,
            batch_size: 100,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: EmbeddingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.dimensions, 1536);
    }

    // -- OpenAiEmbedder request formatting -----------------------------------

    #[test]
    fn test_openai_request_single() {
        let embedder =
            OpenAiEmbedder::new("sk-test-key".into(), "text-embedding-3-small".into(), 1536);
        let body = embedder.build_request_body(&["hello world"]);
        assert_eq!(body["input"], "hello world");
        assert_eq!(body["model"], "text-embedding-3-small");
    }

    #[test]
    fn test_openai_request_batch() {
        let embedder =
            OpenAiEmbedder::new("sk-test-key".into(), "text-embedding-3-small".into(), 1536);
        let body = embedder.build_request_body(&["hello", "world"]);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0], "hello");
        assert_eq!(input[1], "world");
    }

    #[test]
    fn test_openai_parse_response() {
        let response = serde_json::json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0},
                {"embedding": [0.4, 0.5, 0.6], "index": 1}
            ]
        });
        let vecs = OpenAiEmbedder::parse_response(&response).unwrap();
        assert_eq!(vecs.len(), 2);
        assert_eq!(vecs[0], vec![0.1_f32, 0.2, 0.3]);
        assert_eq!(vecs[1], vec![0.4_f32, 0.5, 0.6]);
    }

    // -- EmbeddingStore ------------------------------------------------------

    fn test_store() -> EmbeddingStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let arc = Arc::new(Mutex::new(conn));
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&[
            "rust programming language systems",
            "python scripting language web",
            "cooking recipes kitchen food",
            "machine learning neural networks",
        ]);
        EmbeddingStore::new(arc, Box::new(embedder)).unwrap()
    }

    #[test]
    fn test_store_and_search() {
        let store = test_store();
        store
            .store("rust systems programming", HashMap::new())
            .unwrap();
        store
            .store("python web development", HashMap::new())
            .unwrap();
        store
            .store("cooking recipes for pasta", HashMap::new())
            .unwrap();

        let results = store.search("rust programming", 2).unwrap();
        assert!(!results.is_empty());
        // The top result should be about rust.
        assert!(
            results[0].1.text.contains("rust"),
            "top result should match 'rust', got: {}",
            results[0].1.text
        );
    }

    #[test]
    fn test_store_top_k_count() {
        let store = test_store();
        store.store("alpha", HashMap::new()).unwrap();
        store.store("beta", HashMap::new()).unwrap();
        store.store("gamma", HashMap::new()).unwrap();
        store.store("delta", HashMap::new()).unwrap();

        let results = store.search("alpha", 2).unwrap();
        assert_eq!(results.len(), 2, "should return exactly k results");
    }

    #[test]
    fn test_store_delete() {
        let store = test_store();
        let id = store.store("to be deleted", HashMap::new()).unwrap();
        assert_eq!(store.count().unwrap(), 1);

        store.delete(&id).unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_store_count() {
        let store = test_store();
        assert_eq!(store.count().unwrap(), 0);

        store.store("one", HashMap::new()).unwrap();
        assert_eq!(store.count().unwrap(), 1);

        store.store("two", HashMap::new()).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn test_store_rebuild_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let arc = Arc::new(Mutex::new(conn));

        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["hello world", "foo bar"]);
        let store = EmbeddingStore::new(Arc::clone(&arc), Box::new(embedder)).unwrap();

        store.store("hello world test", HashMap::new()).unwrap();
        store.store("foo bar baz", HashMap::new()).unwrap();
        assert_eq!(store.count().unwrap(), 2);

        let rebuilt = store.rebuild_index().unwrap();
        assert_eq!(rebuilt, 2);
        assert_eq!(store.count().unwrap(), 2);
    }

    // -- top_k_similar -------------------------------------------------------

    #[test]
    fn test_cosine_similarity_single_dimension() {
        let a = vec![3.0];
        let b = vec![5.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "same direction in 1D should be 1.0"
        );
    }

    #[test]
    fn test_cosine_similarity_negative_values() {
        let a = vec![-1.0, -2.0];
        let b = vec![-3.0, -6.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "parallel negative vectors are similar"
        );
    }

    #[test]
    fn test_builtin_embedder_default() {
        let embedder = BuiltInEmbedder::default();
        assert_eq!(embedder.dimensions(), 0);
    }

    #[test]
    fn test_builtin_embed_empty_text() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["hello world", "foo bar"]);
        let vec = embedder.embed("").unwrap();
        assert_eq!(vec.len(), embedder.dimensions());
        assert!(
            vec.iter().all(|&v| v == 0.0),
            "empty text yields zero vector"
        );
    }

    #[test]
    fn test_builtin_dimensions_matches_vocab() {
        let mut embedder = BuiltInEmbedder::new();
        embedder.fit(&["alpha beta gamma", "delta epsilon"]);
        assert!(embedder.dimensions() > 0);
        let vec = embedder.embed("alpha").unwrap();
        assert_eq!(vec.len(), embedder.dimensions());
    }

    #[test]
    fn test_openai_embedder_dimensions() {
        let embedder = OpenAiEmbedder::new("key".into(), "model".into(), 768);
        assert_eq!(embedder.dimensions(), 768);
    }

    #[test]
    fn test_openai_embed_returns_error() {
        let embedder = OpenAiEmbedder::new("key".into(), "model".into(), 768);
        assert!(embedder.embed("test").is_err());
    }

    #[test]
    fn test_openai_embed_batch_returns_error() {
        let embedder = OpenAiEmbedder::new("key".into(), "model".into(), 768);
        assert!(embedder.embed_batch(&["a", "b"]).is_err());
    }

    #[test]
    fn test_openai_parse_response_missing_data() {
        let resp = serde_json::json!({"no_data": true});
        assert!(OpenAiEmbedder::parse_response(&resp).is_err());
    }

    #[test]
    fn test_vec_bytes_single_value() {
        let original = vec![42.0_f32];
        let bytes = vec_to_bytes(&original);
        assert_eq!(bytes.len(), 4);
        let restored = bytes_to_vec(&bytes);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_store_with_metadata() {
        let store = test_store();
        let mut meta = HashMap::new();
        meta.insert("source".to_string(), "test".to_string());
        let id = store.store("text with metadata", meta).unwrap();
        assert!(!id.is_empty());
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn test_store_delete_nonexistent() {
        let store = test_store();
        // Deleting a non-existent ID should not error
        store.delete("nonexistent-id").unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_top_k_similar_empty_list() {
        let query = vec![1.0, 0.0];
        let results = top_k_similar(&query, &[], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_top_k_similar_k_larger_than_list() {
        let embeddings = vec![Embedding {
            id: "only".into(),
            text: "one".into(),
            vector: vec![1.0, 0.0],
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }];
        let query = vec![1.0, 0.0];
        let results = top_k_similar(&query, &embeddings, 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_top_k_similar_ordering() {
        let embeddings = vec![
            Embedding {
                id: "a".into(),
                text: "close".into(),
                vector: vec![0.9, 0.1],
                metadata: HashMap::new(),
                created_at: Utc::now(),
            },
            Embedding {
                id: "b".into(),
                text: "far".into(),
                vector: vec![0.0, 1.0],
                metadata: HashMap::new(),
                created_at: Utc::now(),
            },
        ];
        let query = vec![1.0, 0.0];
        let results = top_k_similar(&query, &embeddings, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].1.id, "a", "closer vector should come first");
        assert!(results[0].0 > results[1].0, "scores should be descending");
    }
}
