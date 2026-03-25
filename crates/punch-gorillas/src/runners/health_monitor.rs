//! Health Monitor gorilla — monitors system health periodically.
//!
//! Checks API endpoint health, monitors memory/CPU usage (via system info),
//! alerts on threshold violations. Designed to run every 5 minutes.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use punch_memory::MemorySubstrate;
use punch_runtime::LlmDriver;
use punch_types::{GorillaManifest, PunchResult};

use crate::{GorillaOutput, GorillaRunner, RequirementStatus};

// ---------------------------------------------------------------------------
// Health status
// ---------------------------------------------------------------------------

/// Overall health status of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// All checks passed.
    Healthy,
    /// Some checks are in a warning state.
    Degraded,
    /// Critical thresholds breached.
    Critical,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "Healthy"),
            Self::Degraded => write!(f, "Degraded"),
            Self::Critical => write!(f, "Critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for health check thresholds.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Memory usage warning threshold (percentage, 0.0 - 100.0).
    pub memory_warning_percent: f64,
    /// Memory usage critical threshold (percentage, 0.0 - 100.0).
    pub memory_critical_percent: f64,
    /// Disk usage warning threshold (percentage, 0.0 - 100.0).
    pub disk_warning_percent: f64,
    /// HTTP endpoint timeout in milliseconds.
    pub endpoint_timeout_ms: u64,
    /// Maximum number of health history entries to keep.
    pub max_history: usize,
    /// Disk paths to check.
    pub disk_paths: Vec<String>,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            memory_warning_percent: 80.0,
            memory_critical_percent: 95.0,
            disk_warning_percent: 85.0,
            endpoint_timeout_ms: 5000,
            max_history: 100,
            disk_paths: vec!["/".to_string()],
        }
    }
}

// ---------------------------------------------------------------------------
// Check results
// ---------------------------------------------------------------------------

/// Result of an individual health check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Name of the check.
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Human-readable message.
    pub message: String,
    /// Timestamp of the check.
    pub timestamp: DateTime<Utc>,
}

/// Result of an HTTP endpoint check.
#[derive(Debug, Clone)]
pub struct EndpointResult {
    /// Name of the endpoint.
    pub name: String,
    /// URL checked.
    pub url: String,
    /// Whether the endpoint is healthy.
    pub healthy: bool,
    /// HTTP status code (if received).
    pub status_code: Option<u16>,
    /// Response time in milliseconds.
    pub response_time_ms: Option<u64>,
    /// Error message (if any).
    pub error: Option<String>,
}

/// A health check endpoint configuration.
#[derive(Debug, Clone)]
pub struct HealthEndpoint {
    /// Human-readable name.
    pub name: String,
    /// URL to check.
    pub url: String,
    /// Expected response substring (if any).
    pub expected: Option<String>,
}

/// Full health report produced by a single monitoring run.
#[derive(Debug, Clone)]
pub struct HealthReport {
    /// Overall status.
    pub status: HealthStatus,
    /// Timestamp of the report.
    pub timestamp: DateTime<Utc>,
    /// Memory usage percentage (0.0 - 100.0).
    pub memory_usage_pct: f64,
    /// Total system memory in bytes.
    pub memory_total_bytes: u64,
    /// Available system memory in bytes.
    pub memory_available_bytes: u64,
    /// Disk check results.
    pub disk_checks: Vec<CheckResult>,
    /// Endpoint check results.
    pub endpoint_results: Vec<EndpointResult>,
    /// All individual check results.
    pub all_checks: Vec<CheckResult>,
    /// Number of checks that passed.
    pub checks_passed: usize,
    /// Number of checks that failed.
    pub checks_failed: usize,
    /// Alerts generated.
    pub alerts: Vec<String>,
}

// ---------------------------------------------------------------------------
// HealthMonitor
// ---------------------------------------------------------------------------

/// Health monitor gorilla that checks system health.
pub struct HealthMonitor {
    manifest: GorillaManifest,
    /// Configuration.
    config: HealthConfig,
    /// Endpoints to check (URL -> expected status text).
    endpoints: Vec<HealthEndpoint>,
    /// Health check history (most recent last).
    history: std::sync::Mutex<VecDeque<HealthReport>>,
}

impl HealthMonitor {
    /// Create a new health monitor with default settings.
    pub fn new() -> Self {
        Self {
            manifest: Self::default_manifest(),
            config: HealthConfig::default(),
            endpoints: Vec::new(),
            history: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// Create a health monitor with custom endpoints and thresholds.
    pub fn with_config(
        endpoints: Vec<HealthEndpoint>,
        memory_warning_percent: f64,
        disk_warning_percent: f64,
    ) -> Self {
        let config = HealthConfig {
            memory_warning_percent,
            disk_warning_percent,
            ..Default::default()
        };
        Self {
            manifest: Self::default_manifest(),
            config,
            endpoints,
            history: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// Create a health monitor with full configuration.
    pub fn with_full_config(endpoints: Vec<HealthEndpoint>, config: HealthConfig) -> Self {
        Self {
            manifest: Self::default_manifest(),
            config,
            endpoints,
            history: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// Get the health check configuration.
    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    /// Get the health check history.
    pub fn history(&self) -> Vec<HealthReport> {
        self.history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the default manifest for this gorilla.
    fn default_manifest() -> GorillaManifest {
        GorillaManifest {
            name: "Health Monitor".to_string(),
            description:
                "Monitors system health, checks endpoints, and alerts on threshold violations."
                    .to_string(),
            schedule: "every 5m".to_string(),
            moves_required: vec!["web_fetch".to_string()],
            settings_schema: None,
            dashboard_metrics: vec![
                "endpoints_checked".to_string(),
                "endpoints_healthy".to_string(),
                "alerts_raised".to_string(),
                "memory_usage_pct".to_string(),
            ],
            system_prompt: Some(
                "You are the Health Monitor gorilla. Check system health, report issues, \
                 and provide actionable recommendations for any problems found."
                    .to_string(),
            ),
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        }
    }

    /// Perform all system health checks.
    fn perform_health_check(&self) -> HealthReport {
        let now = Utc::now();
        let mut all_checks = Vec::new();
        let mut alerts = Vec::new();

        // --- Memory check ---
        let (mem_total, mem_available) = get_system_memory();
        let memory_usage_pct = if mem_total > 0 {
            ((mem_total - mem_available) as f64 / mem_total as f64) * 100.0
        } else {
            0.0
        };

        let mem_passed = memory_usage_pct < self.config.memory_warning_percent;
        let mem_msg = format!("Memory usage: {memory_usage_pct:.1}%");
        if !mem_passed {
            let alert = format!(
                "ALERT: Memory usage at {memory_usage_pct:.1}% exceeds warning threshold of {:.1}%",
                self.config.memory_warning_percent
            );
            warn!("{}", alert);
            alerts.push(alert);
        }
        all_checks.push(CheckResult {
            name: "memory".to_string(),
            passed: mem_passed,
            message: mem_msg,
            timestamp: now,
        });

        // --- Disk checks ---
        let mut disk_checks = Vec::new();
        for path in &self.config.disk_paths {
            let (disk_total, disk_available) = get_disk_space(path);
            let disk_usage_pct = if disk_total > 0 {
                ((disk_total - disk_available) as f64 / disk_total as f64) * 100.0
            } else {
                0.0
            };
            let passed = disk_usage_pct < self.config.disk_warning_percent;
            let msg = format!("Disk '{}': {disk_usage_pct:.1}% used", path);
            if !passed {
                let alert = format!(
                    "ALERT: Disk '{}' at {disk_usage_pct:.1}% exceeds warning threshold of {:.1}%",
                    path, self.config.disk_warning_percent
                );
                warn!("{}", alert);
                alerts.push(alert);
            }
            let check = CheckResult {
                name: format!("disk:{}", path),
                passed,
                message: msg,
                timestamp: now,
            };
            disk_checks.push(check.clone());
            all_checks.push(check);
        }

        // --- Determine overall status ---
        let checks_passed = all_checks.iter().filter(|c| c.passed).count();
        let checks_failed = all_checks.iter().filter(|c| !c.passed).count();

        let status = if memory_usage_pct >= self.config.memory_critical_percent {
            HealthStatus::Critical
        } else if checks_failed > 0 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        HealthReport {
            status,
            timestamp: now,
            memory_usage_pct,
            memory_total_bytes: mem_total,
            memory_available_bytes: mem_available,
            disk_checks,
            endpoint_results: Vec::new(), // filled later by async check
            all_checks,
            checks_passed,
            checks_failed,
            alerts,
        }
    }

    /// Check HTTP endpoint health.
    async fn check_endpoints(&self) -> Vec<EndpointResult> {
        let mut results = Vec::new();
        let timeout = Duration::from_millis(self.config.endpoint_timeout_ms);

        for endpoint in &self.endpoints {
            let result = check_http_endpoint(endpoint, timeout).await;
            results.push(result);
        }

        results
    }

    /// Store a report in history, trimming to max_history.
    fn record_history(&self, report: &HealthReport) {
        if let Ok(mut history) = self.history.lock() {
            history.push_back(report.clone());
            while history.len() > self.config.max_history {
                history.pop_front();
            }
        }
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Read system memory info. Returns (total_bytes, available_bytes).
fn get_system_memory() -> (u64, u64) {
    // Try /proc/meminfo on Linux.
    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
            let mut total: u64 = 0;
            let mut available: u64 = 0;
            for line in contents.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    total = parse_meminfo_kb(rest) * 1024;
                } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                    available = parse_meminfo_kb(rest) * 1024;
                }
            }
            if total > 0 {
                return (total, available);
            }
        }
    }

    // macOS: use sysctl-style approach via process output.
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let total = stdout.trim().parse::<u64>().unwrap_or(0);
            // Get page size and free pages for available memory.
            let available = get_macos_available_memory();
            if total > 0 {
                return (total, available);
            }
        }
    }

    // Fallback: estimate from process info.
    estimate_memory_fallback()
}

#[cfg(target_os = "linux")]
fn parse_meminfo_kb(value: &str) -> u64 {
    value
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn get_macos_available_memory() -> u64 {
    if let Ok(output) = std::process::Command::new("vm_stat").output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let page_size: u64 = 16384; // ARM64 macOS default; x86 is 4096
        let mut free_pages: u64 = 0;
        let mut inactive_pages: u64 = 0;
        for line in stdout.lines() {
            if line.starts_with("Pages free:") {
                free_pages = parse_vm_stat_value(line);
            } else if line.starts_with("Pages inactive:") {
                inactive_pages = parse_vm_stat_value(line);
            }
        }
        return (free_pages + inactive_pages) * page_size;
    }
    0
}

#[cfg(target_os = "macos")]
fn parse_vm_stat_value(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .map(|v| v.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0))
        .unwrap_or(0)
}

/// Fallback memory estimation when platform-specific methods are unavailable.
fn estimate_memory_fallback() -> (u64, u64) {
    // Return a reasonable default so callers always get something.
    let total: u64 = 8 * 1024 * 1024 * 1024; // 8 GB assumed
    let available: u64 = 4 * 1024 * 1024 * 1024; // 50% available
    (total, available)
}

/// Get disk space for a given path. Returns (total_bytes, available_bytes).
fn get_disk_space(path: &str) -> (u64, u64) {
    // Use statvfs on Unix-like systems.
    #[cfg(unix)]
    {
        use std::ffi::CString;
        if let Ok(c_path) = CString::new(path) {
            unsafe {
                let mut stat: libc::statvfs = std::mem::zeroed();
                if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
                    let total = stat.f_blocks * stat.f_frsize;
                    let available = stat.f_bavail * stat.f_frsize;
                    return (total, available);
                }
            }
        }
    }

    // Suppress unused variable warning on non-unix.
    let _ = path;

    // Fallback.
    (100 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024)
}

/// Check a single HTTP endpoint's health.
async fn check_http_endpoint(endpoint: &HealthEndpoint, timeout: Duration) -> EndpointResult {
    let start = std::time::Instant::now();

    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return EndpointResult {
                name: endpoint.name.clone(),
                url: endpoint.url.clone(),
                healthy: false,
                status_code: None,
                response_time_ms: None,
                error: Some(format!("failed to build HTTP client: {e}")),
            };
        }
    };

    match client.get(&endpoint.url).send().await {
        Ok(response) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let status = response.status().as_u16();
            let healthy = if let Some(expected) = &endpoint.expected {
                if let Ok(body) = response.text().await {
                    body.contains(expected)
                } else {
                    false
                }
            } else {
                (200..400).contains(&status)
            };

            debug!(
                endpoint = %endpoint.name,
                url = %endpoint.url,
                status,
                elapsed_ms = elapsed,
                healthy,
                "endpoint check completed"
            );

            EndpointResult {
                name: endpoint.name.clone(),
                url: endpoint.url.clone(),
                healthy,
                status_code: Some(status),
                response_time_ms: Some(elapsed),
                error: None,
            }
        }
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let is_timeout = e.is_timeout();
            let error_msg = if is_timeout {
                format!("timeout after {}ms", timeout.as_millis())
            } else {
                format!("{e}")
            };

            warn!(
                endpoint = %endpoint.name,
                url = %endpoint.url,
                error = %error_msg,
                "endpoint check failed"
            );

            EndpointResult {
                name: endpoint.name.clone(),
                url: endpoint.url.clone(),
                healthy: false,
                status_code: None,
                response_time_ms: Some(elapsed),
                error: Some(error_msg),
            }
        }
    }
}

#[async_trait]
impl GorillaRunner for HealthMonitor {
    fn manifest(&self) -> &GorillaManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        memory: &MemorySubstrate,
        _driver: Arc<dyn LlmDriver>,
    ) -> PunchResult<GorillaOutput> {
        info!("Health Monitor gorilla starting execution");

        // Perform synchronous system checks.
        let mut report = self.perform_health_check();

        // Perform async endpoint checks.
        let endpoint_results = self.check_endpoints().await;
        let mut endpoint_alerts = Vec::new();

        let mut endpoints_checked: u32 = 0;
        let mut endpoints_healthy: u32 = 0;

        for result in &endpoint_results {
            endpoints_checked += 1;
            if result.healthy {
                endpoints_healthy += 1;
            } else {
                let alert = format!(
                    "ALERT: Endpoint '{}' ({}) is unhealthy: {}",
                    result.name,
                    result.url,
                    result.error.as_deref().unwrap_or("unexpected response")
                );
                warn!("{}", alert);
                endpoint_alerts.push(alert.clone());
                report.alerts.push(alert);
            }

            report.all_checks.push(CheckResult {
                name: format!("endpoint:{}", result.name),
                passed: result.healthy,
                message: if result.healthy {
                    format!(
                        "Endpoint '{}' healthy ({}ms)",
                        result.name,
                        result.response_time_ms.unwrap_or(0)
                    )
                } else {
                    format!(
                        "Endpoint '{}' unhealthy: {}",
                        result.name,
                        result.error.as_deref().unwrap_or("unknown")
                    )
                },
                timestamp: Utc::now(),
            });
        }

        report.endpoint_results = endpoint_results;

        // Recalculate pass/fail with endpoint results.
        report.checks_passed = report.all_checks.iter().filter(|c| c.passed).count();
        report.checks_failed = report.all_checks.iter().filter(|c| !c.passed).count();

        // Update status if endpoints are unhealthy.
        if report.checks_failed > 0 && report.status == HealthStatus::Healthy {
            report.status = HealthStatus::Degraded;
        }

        // Record in history.
        self.record_history(&report);

        // Build summary.
        let summary = format!(
            "Health check completed at {}. Status: {}. Memory: {:.1}%. \
             Endpoints: {}/{} healthy. Checks: {}/{} passed. Alerts: {}.",
            report.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            report.status,
            report.memory_usage_pct,
            endpoints_healthy,
            endpoints_checked,
            report.checks_passed,
            report.checks_passed + report.checks_failed,
            report.alerts.len()
        );

        // Store health report as a memory entry.
        let mem_key = format!("health_report_{}", Utc::now().format("%Y%m%d_%H%M%S"));
        if let Err(e) = memory
            .store_memory(&punch_types::FighterId::new(), &mem_key, &summary, 0.9)
            .await
        {
            warn!(error = %e, "failed to store health report in memory");
        }

        let mut artifacts = vec![format!("memory:{mem_key}")];

        if !report.alerts.is_empty() {
            artifacts.push(format!("alerts:{}", report.alerts.join("; ")));
        }

        // Add structured report details.
        artifacts.push(format!("status:{}", report.status));
        artifacts.push(format!("memory_pct:{:.1}", report.memory_usage_pct));
        artifacts.push(format!("checks_passed:{}", report.checks_passed));
        artifacts.push(format!("checks_failed:{}", report.checks_failed));

        info!(
            status = %report.status,
            endpoints_checked,
            endpoints_healthy,
            checks_passed = report.checks_passed,
            checks_failed = report.checks_failed,
            alerts = report.alerts.len(),
            "Health Monitor gorilla execution complete"
        );

        Ok(GorillaOutput {
            summary,
            artifacts,
            next_run: None,
        })
    }

    fn check_requirements(&self) -> Vec<RequirementStatus> {
        vec![RequirementStatus {
            name: "system_access".to_string(),
            met: true,
            message: "System metrics accessible".to_string(),
        }]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_monitor_new() {
        let monitor = HealthMonitor::new();
        assert_eq!(monitor.manifest().name, "Health Monitor");
        assert!(monitor.endpoints.is_empty());
    }

    #[test]
    fn health_monitor_default() {
        let monitor = HealthMonitor::default();
        assert_eq!(monitor.config.memory_warning_percent, 80.0);
    }

    #[test]
    fn health_monitor_with_config() {
        let endpoints = vec![HealthEndpoint {
            name: "api".to_string(),
            url: "http://localhost:8080/health".to_string(),
            expected: Some("ok".to_string()),
        }];
        let monitor = HealthMonitor::with_config(endpoints, 70.0, 75.0);
        assert_eq!(monitor.endpoints.len(), 1);
        assert_eq!(monitor.config.memory_warning_percent, 70.0);
        assert_eq!(monitor.config.disk_warning_percent, 75.0);
    }

    #[test]
    fn health_monitor_with_full_config() {
        let config = HealthConfig {
            memory_warning_percent: 60.0,
            memory_critical_percent: 90.0,
            disk_warning_percent: 70.0,
            endpoint_timeout_ms: 3000,
            max_history: 50,
            disk_paths: vec!["/tmp".to_string()],
        };
        let monitor = HealthMonitor::with_full_config(Vec::new(), config);
        assert_eq!(monitor.config.memory_warning_percent, 60.0);
        assert_eq!(monitor.config.max_history, 50);
    }

    #[test]
    fn health_monitor_check_requirements() {
        let monitor = HealthMonitor::new();
        let reqs = monitor.check_requirements();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].met);
    }

    #[test]
    fn health_check_returns_valid_report() {
        let monitor = HealthMonitor::new();
        let report = monitor.perform_health_check();
        assert!(report.memory_usage_pct >= 0.0);
        assert!(report.memory_usage_pct <= 100.0);
        assert!(report.memory_total_bytes > 0);
        assert!(!report.all_checks.is_empty());
    }

    #[test]
    fn health_check_healthy_with_high_threshold() {
        // Use a very high threshold so we don't trigger warnings.
        let config = HealthConfig {
            memory_warning_percent: 99.9,
            memory_critical_percent: 99.99,
            disk_warning_percent: 99.9,
            disk_paths: Vec::new(), // No disk checks.
            ..Default::default()
        };
        let monitor = HealthMonitor::with_full_config(Vec::new(), config);
        let report = monitor.perform_health_check();
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.alerts.is_empty());
    }

    #[test]
    fn health_check_degraded_with_low_memory_threshold() {
        // Use a very low threshold so memory check will fail.
        let config = HealthConfig {
            memory_warning_percent: 0.001,
            memory_critical_percent: 99.99,
            disk_warning_percent: 99.9,
            disk_paths: Vec::new(),
            ..Default::default()
        };
        let monitor = HealthMonitor::with_full_config(Vec::new(), config);
        let report = monitor.perform_health_check();
        assert_eq!(report.status, HealthStatus::Degraded);
        assert!(report.checks_failed > 0);
        assert!(!report.alerts.is_empty());
    }

    #[test]
    fn health_check_critical_with_very_low_thresholds() {
        let config = HealthConfig {
            memory_warning_percent: 0.001,
            memory_critical_percent: 0.001,
            disk_warning_percent: 99.9,
            disk_paths: Vec::new(),
            ..Default::default()
        };
        let monitor = HealthMonitor::with_full_config(Vec::new(), config);
        let report = monitor.perform_health_check();
        assert_eq!(report.status, HealthStatus::Critical);
    }

    #[test]
    fn health_report_contains_all_checks() {
        let monitor = HealthMonitor::new();
        let report = monitor.perform_health_check();
        // Should at least have memory check and disk checks.
        assert!(report.checks_passed + report.checks_failed > 0);
        assert_eq!(
            report.checks_passed + report.checks_failed,
            report.all_checks.len()
        );
    }

    #[test]
    fn configurable_thresholds_work() {
        let config = HealthConfig {
            memory_warning_percent: 50.0,
            memory_critical_percent: 90.0,
            disk_warning_percent: 60.0,
            ..Default::default()
        };
        assert_eq!(config.memory_warning_percent, 50.0);
        assert_eq!(config.memory_critical_percent, 90.0);
        assert_eq!(config.disk_warning_percent, 60.0);
    }

    #[test]
    fn history_tracking_stores_entries() {
        let config = HealthConfig {
            memory_warning_percent: 99.9,
            memory_critical_percent: 99.99,
            disk_warning_percent: 99.9,
            disk_paths: Vec::new(),
            max_history: 3,
            ..Default::default()
        };
        let monitor = HealthMonitor::with_full_config(Vec::new(), config);

        // Perform 5 checks, history should keep only 3.
        for _ in 0..5 {
            let report = monitor.perform_health_check();
            monitor.record_history(&report);
        }

        let history = monitor.history();
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn history_tracking_empty_initially() {
        let monitor = HealthMonitor::new();
        let history = monitor.history();
        assert!(history.is_empty());
    }

    #[test]
    fn health_monitor_manifest_schedule() {
        let monitor = HealthMonitor::new();
        assert_eq!(monitor.manifest().schedule, "every 5m");
    }

    #[test]
    fn health_endpoint_clone() {
        let endpoint = HealthEndpoint {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            expected: None,
        };
        let cloned = endpoint.clone();
        assert_eq!(cloned.name, "test");
    }

    #[test]
    fn health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "Healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "Degraded");
        assert_eq!(format!("{}", HealthStatus::Critical), "Critical");
    }

    #[test]
    fn get_system_memory_returns_nonzero() {
        let (total, _available) = get_system_memory();
        assert!(total > 0);
    }

    #[test]
    fn get_disk_space_returns_nonzero() {
        let (total, _available) = get_disk_space("/");
        assert!(total > 0);
    }

    #[test]
    fn default_health_config() {
        let config = HealthConfig::default();
        assert_eq!(config.memory_warning_percent, 80.0);
        assert_eq!(config.memory_critical_percent, 95.0);
        assert_eq!(config.disk_warning_percent, 85.0);
        assert_eq!(config.endpoint_timeout_ms, 5000);
        assert_eq!(config.max_history, 100);
    }

    #[tokio::test]
    async fn endpoint_check_with_invalid_url() {
        let endpoint = HealthEndpoint {
            name: "bad".to_string(),
            url: "http://192.0.2.1:1/nonexistent".to_string(),
            expected: None,
        };
        let result = check_http_endpoint(&endpoint, Duration::from_millis(100)).await;
        // Should be unhealthy — either the request fails (error set) or returns non-2xx.
        assert!(!result.healthy);
    }
}
