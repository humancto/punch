//! Lightweight Prometheus-compatible metrics registry.
//!
//! Provides thread-safe counters, gauges, and histograms without heavy external
//! dependencies. All metric types use atomics and [`DashMap`] for lock-free
//! concurrent access. The registry can export its state in Prometheus text
//! exposition format via [`MetricsRegistry::export_prometheus`].

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use dashmap::DashMap;

// ---------------------------------------------------------------------------
// Histogram
// ---------------------------------------------------------------------------

/// Default histogram buckets (seconds) — modelled after the Prometheus default
/// plus some finer-grained sub-millisecond buckets useful for API latency.
const DEFAULT_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// A histogram that tracks the distribution of observed values across
/// pre-defined buckets.
///
/// Each bucket counter and the running sum/count are stored as atomics so
/// that recording an observation never blocks readers.
#[derive(Debug)]
pub struct Histogram {
    /// Upper-bound (inclusive) for each bucket.
    buckets: Vec<f64>,
    /// Cumulative count for each bucket.  `counts[i]` is the number of
    /// observations ≤ `buckets[i]`.
    counts: Vec<AtomicU64>,
    /// Running sum of all observed values (stored as `f64` bits).
    sum_bits: AtomicU64,
    /// Total number of observations.
    count: AtomicU64,
}

impl Histogram {
    /// Create a new histogram with the given bucket upper bounds.
    ///
    /// The buckets are automatically sorted and de-duplicated.
    pub fn new(mut buckets: Vec<f64>) -> Self {
        buckets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        buckets.dedup();
        let counts = buckets.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            buckets,
            counts,
            sum_bits: AtomicU64::new(f64::to_bits(0.0)),
            count: AtomicU64::new(0),
        }
    }

    /// Create a histogram with the default bucket boundaries.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_BUCKETS.to_vec())
    }

    /// Record an observation.
    pub fn observe(&self, value: f64) {
        // Increment bucket counts (cumulative).
        for (i, upper) in self.buckets.iter().enumerate() {
            if value <= *upper {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }

        // Atomically add to the running sum using a CAS loop.
        loop {
            let old_bits = self.sum_bits.load(Ordering::Relaxed);
            let old = f64::from_bits(old_bits);
            let new = old + value;
            let new_bits = f64::to_bits(new);
            if self
                .sum_bits
                .compare_exchange_weak(old_bits, new_bits, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the total count of observations.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Get the running sum of all observations.
    pub fn sum(&self) -> f64 {
        f64::from_bits(self.sum_bits.load(Ordering::Relaxed))
    }

    /// Return `(upper_bound, cumulative_count)` for each bucket.
    pub fn bucket_counts(&self) -> Vec<(f64, u64)> {
        self.buckets
            .iter()
            .zip(self.counts.iter())
            .map(|(b, c)| (*b, c.load(Ordering::Relaxed)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// MetricsRegistry
// ---------------------------------------------------------------------------

/// A lightweight, thread-safe metrics registry that can export its state in
/// Prometheus text format.
///
/// Metrics are keyed by a full name that includes labels encoded as
/// `name{label1="val1",label2="val2"}`. The helper methods
/// [`counter_with_labels`](MetricsRegistry::counter_with_labels) etc. build
/// these keys automatically.
#[derive(Debug)]
pub struct MetricsRegistry {
    /// Monotonically increasing counters.
    counters: DashMap<String, AtomicU64>,
    /// Point-in-time gauges (may go up or down).
    gauges: DashMap<String, AtomicI64>,
    /// Histograms tracking value distributions.
    histograms: DashMap<String, Histogram>,
    /// Help text for each metric base name.
    help: DashMap<String, String>,
    /// Type for each metric base name (`counter`, `gauge`, `histogram`).
    metric_type: DashMap<String, String>,
}

impl MetricsRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            counters: DashMap::new(),
            gauges: DashMap::new(),
            histograms: DashMap::new(),
            help: DashMap::new(),
            metric_type: DashMap::new(),
        }
    }

    // -- Registration helpers ------------------------------------------------

    /// Register a counter with help text.
    pub fn register_counter(&self, name: &str, help: &str) {
        self.help.insert(name.to_string(), help.to_string());
        self.metric_type
            .insert(name.to_string(), "counter".to_string());
    }

    /// Register a gauge with help text.
    pub fn register_gauge(&self, name: &str, help: &str) {
        self.help.insert(name.to_string(), help.to_string());
        self.metric_type
            .insert(name.to_string(), "gauge".to_string());
    }

    /// Register a histogram with help text.
    pub fn register_histogram(&self, name: &str, help: &str) {
        self.help.insert(name.to_string(), help.to_string());
        self.metric_type
            .insert(name.to_string(), "histogram".to_string());
    }

    // -- Counter operations --------------------------------------------------

    /// Increment a counter by 1.
    pub fn counter_inc(&self, key: &str) {
        self.counter_add(key, 1);
    }

    /// Increment a counter by `n`.
    pub fn counter_add(&self, key: &str, n: u64) {
        self.counters
            .entry(key.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(n, Ordering::Relaxed);
    }

    /// Build a label-encoded key and increment the counter by 1.
    pub fn counter_with_labels(&self, name: &str, labels: &[(&str, &str)]) {
        let key = encode_key(name, labels);
        self.counter_inc(&key);
    }

    /// Build a label-encoded key and add `n` to the counter.
    pub fn counter_add_with_labels(&self, name: &str, labels: &[(&str, &str)], n: u64) {
        let key = encode_key(name, labels);
        self.counter_add(&key, n);
    }

    /// Get the current value of a counter.
    pub fn counter_get(&self, key: &str) -> u64 {
        self.counters
            .get(key)
            .map(|v| v.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    // -- Gauge operations ----------------------------------------------------

    /// Set a gauge to an absolute value.
    pub fn gauge_set(&self, key: &str, value: i64) {
        self.gauges
            .entry(key.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .store(value, Ordering::Relaxed);
    }

    /// Increment a gauge by 1.
    pub fn gauge_inc(&self, key: &str) {
        self.gauge_add(key, 1);
    }

    /// Decrement a gauge by 1.
    pub fn gauge_dec(&self, key: &str) {
        self.gauge_add(key, -1);
    }

    /// Add a signed value to a gauge.
    pub fn gauge_add(&self, key: &str, delta: i64) {
        self.gauges
            .entry(key.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .fetch_add(delta, Ordering::Relaxed);
    }

    /// Get the current value of a gauge.
    pub fn gauge_get(&self, key: &str) -> i64 {
        self.gauges
            .get(key)
            .map(|v| v.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    // -- Histogram operations ------------------------------------------------

    /// Record a value in a histogram, creating it with default buckets if it
    /// does not yet exist.
    pub fn histogram_observe(&self, key: &str, value: f64) {
        self.histograms
            .entry(key.to_string())
            .or_insert_with(Histogram::with_defaults)
            .observe(value);
    }

    /// Record a value in a histogram with labels.
    pub fn histogram_observe_with_labels(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let key = encode_key(name, labels);
        self.histogram_observe(&key, value);
    }

    /// Get a histogram's snapshot: `(buckets, sum, count)`.
    #[allow(clippy::type_complexity)]
    pub fn histogram_get(&self, key: &str) -> Option<(Vec<(f64, u64)>, f64, u64)> {
        self.histograms
            .get(key)
            .map(|h| (h.bucket_counts(), h.sum(), h.count()))
    }

    // -- Prometheus export ---------------------------------------------------

    /// Export all metrics in Prometheus text exposition format.
    pub fn export_prometheus(&self) -> String {
        let mut out = String::new();

        // Collect all base metric names with their type info, then render
        // each metric family grouped together.

        // --- Counters ---
        let counter_families: DashMap<String, Vec<(String, u64)>> = DashMap::new();
        for entry in self.counters.iter() {
            let full_key = entry.key().clone();
            let value = entry.value().load(Ordering::Relaxed);
            let (base, _labels) = split_key(&full_key);
            counter_families
                .entry(base.to_string())
                .or_default()
                .push((full_key, value));
        }

        let mut counter_bases: Vec<String> =
            counter_families.iter().map(|e| e.key().clone()).collect();
        counter_bases.sort();

        for base in &counter_bases {
            if let Some(help) = self.help.get(base.as_str()) {
                out.push_str(&format!("# HELP {} {}\n", base, help.value()));
            }
            out.push_str(&format!("# TYPE {} counter\n", base));

            if let Some(entries) = counter_families.get(base) {
                let mut sorted: Vec<_> = entries.value().clone();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, val) in &sorted {
                    out.push_str(&format!("{} {}\n", key, val));
                }
            }
            out.push('\n');
        }

        // --- Gauges ---
        let gauge_families: DashMap<String, Vec<(String, i64)>> = DashMap::new();
        for entry in self.gauges.iter() {
            let full_key = entry.key().clone();
            let value = entry.value().load(Ordering::Relaxed);
            let (base, _labels) = split_key(&full_key);
            gauge_families
                .entry(base.to_string())
                .or_default()
                .push((full_key, value));
        }

        let mut gauge_bases: Vec<String> = gauge_families.iter().map(|e| e.key().clone()).collect();
        gauge_bases.sort();

        for base in &gauge_bases {
            if let Some(help) = self.help.get(base.as_str()) {
                out.push_str(&format!("# HELP {} {}\n", base, help.value()));
            }
            out.push_str(&format!("# TYPE {} gauge\n", base));

            if let Some(entries) = gauge_families.get(base) {
                let mut sorted: Vec<_> = entries.value().clone();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, val) in &sorted {
                    out.push_str(&format!("{} {}\n", key, val));
                }
            }
            out.push('\n');
        }

        // --- Histograms ---
        let histogram_families: DashMap<String, Vec<String>> = DashMap::new();
        for entry in self.histograms.iter() {
            let full_key = entry.key().clone();
            let (base, _labels) = split_key(&full_key);
            histogram_families
                .entry(base.to_string())
                .or_default()
                .push(full_key);
        }

        let mut hist_bases: Vec<String> =
            histogram_families.iter().map(|e| e.key().clone()).collect();
        hist_bases.sort();

        for base in &hist_bases {
            if let Some(help) = self.help.get(base.as_str()) {
                out.push_str(&format!("# HELP {} {}\n", base, help.value()));
            }
            out.push_str(&format!("# TYPE {} histogram\n", base));

            if let Some(keys) = histogram_families.get(base) {
                let mut sorted_keys = keys.value().clone();
                sorted_keys.sort();

                for key in &sorted_keys {
                    if let Some(h) = self.histograms.get(key.as_str()) {
                        let (_, labels_part) = split_key(key);
                        let label_prefix = if labels_part.is_empty() {
                            String::new()
                        } else {
                            // Strip surrounding braces for re-assembly.
                            let inner = &labels_part[1..labels_part.len() - 1];
                            format!("{},", inner)
                        };

                        for (bound, count) in h.bucket_counts() {
                            out.push_str(&format!(
                                "{}_bucket{{{}le=\"{}\"}} {}\n",
                                base,
                                label_prefix,
                                format_float(bound),
                                count
                            ));
                        }
                        // +Inf bucket = total count
                        out.push_str(&format!(
                            "{}_bucket{{{}le=\"+Inf\"}} {}\n",
                            base,
                            label_prefix,
                            h.count()
                        ));
                        out.push_str(&format!("{}_sum {}\n", key, format_float(h.sum())));
                        out.push_str(&format!("{}_count {}\n", key, h.count()));
                    }
                }
            }
            out.push('\n');
        }

        out
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Well-known metric names
// ---------------------------------------------------------------------------

// Counters
/// Total API requests, labeled by method + path + status.
pub const REQUESTS_TOTAL: &str = "punch_requests_total";
/// Total LLM calls, labeled by provider + model.
pub const LLM_CALLS_TOTAL: &str = "punch_llm_calls_total";
/// Total tool executions, labeled by tool_name + result.
pub const TOOL_EXECUTIONS_TOTAL: &str = "punch_tool_executions_total";
/// Fighters spawned.
pub const FIGHTER_SPAWNS_TOTAL: &str = "punch_fighter_spawns_total";
/// Gorilla runs.
pub const GORILLA_RUNS_TOTAL: &str = "punch_gorilla_runs_total";
/// Messages processed.
pub const MESSAGES_TOTAL: &str = "punch_messages_total";
/// Errors by type.
pub const ERRORS_TOTAL: &str = "punch_errors_total";
/// Total input tokens consumed.
pub const TOKENS_INPUT_TOTAL: &str = "punch_tokens_input_total";
/// Total output tokens consumed.
pub const TOKENS_OUTPUT_TOTAL: &str = "punch_tokens_output_total";

// Gauges
/// Currently active fighters.
pub const ACTIVE_FIGHTERS: &str = "punch_active_fighters";
/// Currently active gorillas.
pub const ACTIVE_GORILLAS: &str = "punch_active_gorillas";
/// Open bout sessions.
pub const ACTIVE_BOUTS: &str = "punch_active_bouts";
/// Total memory entries.
pub const MEMORY_ENTRIES: &str = "punch_memory_entries";
/// Task queue depth.
pub const QUEUE_DEPTH: &str = "punch_queue_depth";

// Histograms
/// API request latency.
pub const REQUEST_DURATION_SECONDS: &str = "punch_request_duration_seconds";
/// LLM call latency.
pub const LLM_LATENCY_SECONDS: &str = "punch_llm_latency_seconds";
/// Tool execution time.
pub const TOOL_EXECUTION_SECONDS: &str = "punch_tool_execution_seconds";

/// Register all well-known Punch metrics with help text.
pub fn register_default_metrics(registry: &MetricsRegistry) {
    // Counters
    registry.register_counter(REQUESTS_TOTAL, "Total API requests");
    registry.register_counter(LLM_CALLS_TOTAL, "Total LLM calls");
    registry.register_counter(TOOL_EXECUTIONS_TOTAL, "Total tool executions");
    registry.register_counter(FIGHTER_SPAWNS_TOTAL, "Total fighters spawned");
    registry.register_counter(GORILLA_RUNS_TOTAL, "Total gorilla executions");
    registry.register_counter(MESSAGES_TOTAL, "Total messages processed");
    registry.register_counter(ERRORS_TOTAL, "Total errors by type");
    registry.register_counter(TOKENS_INPUT_TOTAL, "Total input tokens consumed");
    registry.register_counter(TOKENS_OUTPUT_TOTAL, "Total output tokens consumed");

    // Gauges
    registry.register_gauge(ACTIVE_FIGHTERS, "Currently active fighters");
    registry.register_gauge(ACTIVE_GORILLAS, "Currently rampaging gorillas");
    registry.register_gauge(ACTIVE_BOUTS, "Open bout sessions");
    registry.register_gauge(MEMORY_ENTRIES, "Total memory entries");
    registry.register_gauge(QUEUE_DEPTH, "Task queue depth");

    // Histograms
    registry.register_histogram(REQUEST_DURATION_SECONDS, "API request latency");
    registry.register_histogram(LLM_LATENCY_SECONDS, "LLM call latency");
    registry.register_histogram(TOOL_EXECUTION_SECONDS, "Tool execution time");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode a metric name with labels: `name{k1="v1",k2="v2"}`.
///
/// Returns just `name` when labels are empty.
fn encode_key(name: &str, labels: &[(&str, &str)]) -> String {
    if labels.is_empty() {
        return name.to_string();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect();
    format!("{}{{{}}}", name, parts.join(","))
}

/// Split a full key into `(base_name, labels_with_braces)`.
///
/// `"foo{a=\"1\"}"` → `("foo", "{a=\"1\"}")`.
/// `"foo"` → `("foo", "")`.
fn split_key(key: &str) -> (&str, &str) {
    match key.find('{') {
        Some(idx) => (&key[..idx], &key[idx..]),
        None => (key, ""),
    }
}

/// Format a float for Prometheus output, avoiding unnecessary trailing zeros
/// while keeping at least one decimal place for fractional values.
fn format_float(v: f64) -> String {
    if v == f64::INFINITY {
        return "+Inf".to_string();
    }
    if v == f64::NEG_INFINITY {
        return "-Inf".to_string();
    }
    if v.fract() == 0.0 {
        // Integer value — keep it clean.
        format!("{}", v as i64)
    } else {
        // Use enough precision to be lossless for common values.
        let s = format!("{}", v);
        s
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_counter_increment() {
        let reg = MetricsRegistry::new();
        reg.counter_inc("test_counter");
        assert_eq!(reg.counter_get("test_counter"), 1);
        reg.counter_inc("test_counter");
        assert_eq!(reg.counter_get("test_counter"), 2);
    }

    #[test]
    fn test_counter_add() {
        let reg = MetricsRegistry::new();
        reg.counter_add("test_counter", 10);
        assert_eq!(reg.counter_get("test_counter"), 10);
        reg.counter_add("test_counter", 5);
        assert_eq!(reg.counter_get("test_counter"), 15);
    }

    #[test]
    fn test_gauge_set_inc_dec() {
        let reg = MetricsRegistry::new();
        reg.gauge_set("test_gauge", 42);
        assert_eq!(reg.gauge_get("test_gauge"), 42);

        reg.gauge_inc("test_gauge");
        assert_eq!(reg.gauge_get("test_gauge"), 43);

        reg.gauge_dec("test_gauge");
        assert_eq!(reg.gauge_get("test_gauge"), 42);

        reg.gauge_add("test_gauge", -10);
        assert_eq!(reg.gauge_get("test_gauge"), 32);
    }

    #[test]
    fn test_histogram_observe_and_buckets() {
        let reg = MetricsRegistry::new();
        reg.register_histogram("test_hist", "test histogram");

        // Observe some values.
        reg.histogram_observe("test_hist", 0.003); // <= 0.005
        reg.histogram_observe("test_hist", 0.007); // <= 0.01
        reg.histogram_observe("test_hist", 0.02); // <= 0.025
        reg.histogram_observe("test_hist", 5.5); // <= 10.0

        let (buckets, sum, count) = reg.histogram_get("test_hist").unwrap();
        assert_eq!(count, 4);

        // sum should be approximately 0.003 + 0.007 + 0.02 + 5.5 = 5.53
        let expected_sum = 0.003 + 0.007 + 0.02 + 5.5;
        assert!((sum - expected_sum).abs() < 1e-10);

        // Check cumulative bucket counts.
        // 0.005 bucket: 1 (0.003)
        assert_eq!(buckets[0], (0.005, 1));
        // 0.01 bucket: 2 (0.003, 0.007)
        assert_eq!(buckets[1], (0.01, 2));
        // 0.025 bucket: 3 (0.003, 0.007, 0.02)
        assert_eq!(buckets[2], (0.025, 3));
        // 5.0 bucket: 3 (not 5.5)
        assert_eq!(buckets[9], (5.0, 3));
        // 10.0 bucket: 4 (all)
        assert_eq!(buckets[10], (10.0, 4));
    }

    #[test]
    fn test_labeled_metrics() {
        let reg = MetricsRegistry::new();
        reg.register_counter("http_requests_total", "Total HTTP requests");

        reg.counter_with_labels(
            "http_requests_total",
            &[("method", "GET"), ("status", "200")],
        );
        reg.counter_with_labels(
            "http_requests_total",
            &[("method", "POST"), ("status", "200")],
        );
        reg.counter_with_labels(
            "http_requests_total",
            &[("method", "GET"), ("status", "200")],
        );

        assert_eq!(
            reg.counter_get("http_requests_total{method=\"GET\",status=\"200\"}"),
            2
        );
        assert_eq!(
            reg.counter_get("http_requests_total{method=\"POST\",status=\"200\"}"),
            1
        );
    }

    #[test]
    fn test_prometheus_text_format_counter() {
        let reg = MetricsRegistry::new();
        reg.register_counter("punch_requests_total", "Total API requests");

        reg.counter_with_labels(
            "punch_requests_total",
            &[("method", "GET"), ("status", "200")],
        );

        let output = reg.export_prometheus();
        assert!(output.contains("# HELP punch_requests_total Total API requests"));
        assert!(output.contains("# TYPE punch_requests_total counter"));
        assert!(output.contains("punch_requests_total{method=\"GET\",status=\"200\"} 1"));
    }

    #[test]
    fn test_prometheus_text_format_gauge() {
        let reg = MetricsRegistry::new();
        reg.register_gauge("punch_active_fighters", "Currently active fighters");
        reg.gauge_set("punch_active_fighters", 5);

        let output = reg.export_prometheus();
        assert!(output.contains("# HELP punch_active_fighters Currently active fighters"));
        assert!(output.contains("# TYPE punch_active_fighters gauge"));
        assert!(output.contains("punch_active_fighters 5"));
    }

    #[test]
    fn test_prometheus_text_format_histogram() {
        let reg = MetricsRegistry::new();
        reg.register_histogram("punch_request_duration_seconds", "API request latency");

        reg.histogram_observe("punch_request_duration_seconds", 0.02);
        reg.histogram_observe("punch_request_duration_seconds", 0.08);

        let output = reg.export_prometheus();
        assert!(output.contains("# HELP punch_request_duration_seconds API request latency"));
        assert!(output.contains("# TYPE punch_request_duration_seconds histogram"));
        assert!(output.contains("punch_request_duration_seconds_bucket{le=\"0.025\"} 1"));
        assert!(output.contains("punch_request_duration_seconds_bucket{le=\"0.1\"} 2"));
        assert!(output.contains("punch_request_duration_seconds_bucket{le=\"+Inf\"} 2"));
        assert!(output.contains("punch_request_duration_seconds_sum"));
        assert!(output.contains("punch_request_duration_seconds_count 2"));
    }

    #[test]
    fn test_concurrent_access() {
        let reg = Arc::new(MetricsRegistry::new());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let r = Arc::clone(&reg);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    r.counter_inc("concurrent_counter");
                    r.gauge_inc("concurrent_gauge");
                    r.histogram_observe("concurrent_hist", 0.1);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(reg.counter_get("concurrent_counter"), 10_000);
        assert_eq!(reg.gauge_get("concurrent_gauge"), 10_000);

        let (_, _, count) = reg.histogram_get("concurrent_hist").unwrap();
        assert_eq!(count, 10_000);
    }

    #[test]
    fn test_zero_value_metrics_display() {
        let reg = MetricsRegistry::new();
        reg.register_counter("zero_counter", "A zero counter");
        // Force creation of the key at zero.
        reg.counter_add("zero_counter", 0);

        let output = reg.export_prometheus();
        assert!(output.contains("zero_counter 0"));
    }

    #[test]
    fn test_histogram_percentile_via_buckets() {
        let reg = MetricsRegistry::new();
        // Use custom buckets for a precise test.
        let hist = Histogram::new(vec![1.0, 5.0, 10.0]);
        reg.histograms.insert("custom_hist".to_string(), hist);

        // Observe values: 0.5 (<=1), 3.0 (<=5), 7.0 (<=10), 7.0 (<=10)
        reg.histogram_observe("custom_hist", 0.5);
        reg.histogram_observe("custom_hist", 3.0);
        reg.histogram_observe("custom_hist", 7.0);
        reg.histogram_observe("custom_hist", 7.0);

        let (buckets, sum, count) = reg.histogram_get("custom_hist").unwrap();
        assert_eq!(count, 4);
        assert!((sum - 17.5).abs() < 1e-10);

        // Bucket 1.0: 1 observation (0.5)
        assert_eq!(buckets[0], (1.0, 1));
        // Bucket 5.0: 2 observations (0.5, 3.0)
        assert_eq!(buckets[1], (5.0, 2));
        // Bucket 10.0: 4 observations (all)
        assert_eq!(buckets[2], (10.0, 4));

        // Approximate p50: 50th percentile => 2nd observation out of 4
        // Bucket 5.0 contains 2 total, so p50 ~ within [1.0, 5.0]
        // Bucket 10.0 contains 4 total, p75 ~ within [5.0, 10.0]
        // This validates the bucket distribution is correct for percentile estimation.
    }

    #[test]
    fn test_encode_key_no_labels() {
        assert_eq!(encode_key("my_metric", &[]), "my_metric");
    }

    #[test]
    fn test_encode_key_with_labels() {
        let key = encode_key("my_metric", &[("a", "1"), ("b", "2")]);
        assert_eq!(key, "my_metric{a=\"1\",b=\"2\"}");
    }

    #[test]
    fn test_split_key() {
        let (base, labels) = split_key("foo{a=\"1\"}");
        assert_eq!(base, "foo");
        assert_eq!(labels, "{a=\"1\"}");

        let (base2, labels2) = split_key("bar");
        assert_eq!(base2, "bar");
        assert_eq!(labels2, "");
    }

    #[test]
    fn test_register_default_metrics() {
        let reg = MetricsRegistry::new();
        register_default_metrics(&reg);

        // Verify help text is registered.
        assert!(reg.help.contains_key(REQUESTS_TOTAL));
        assert!(reg.help.contains_key(ACTIVE_FIGHTERS));
        assert!(reg.help.contains_key(REQUEST_DURATION_SECONDS));

        // Verify metric types.
        assert_eq!(
            reg.metric_type
                .get(REQUESTS_TOTAL)
                .unwrap()
                .value()
                .as_str(),
            "counter"
        );
        assert_eq!(
            reg.metric_type
                .get(ACTIVE_FIGHTERS)
                .unwrap()
                .value()
                .as_str(),
            "gauge"
        );
        assert_eq!(
            reg.metric_type
                .get(REQUEST_DURATION_SECONDS)
                .unwrap()
                .value()
                .as_str(),
            "histogram"
        );
    }
}
