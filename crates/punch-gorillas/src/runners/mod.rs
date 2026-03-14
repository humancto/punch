//! Built-in gorilla runner implementations.
//!
//! Each runner implements the [`GorillaRunner`] trait and performs a specific
//! autonomous task.

pub mod data_sweeper;
pub mod health_monitor;
pub mod report_generator;

pub use data_sweeper::{DataSweeper, SweepReport};
pub use health_monitor::{
    CheckResult, EndpointResult, HealthConfig, HealthEndpoint, HealthMonitor, HealthReport,
    HealthStatus,
};
pub use report_generator::{
    FighterSummary, Report, ReportGenerator, ReportMetrics, ReportPeriod, ReportSection,
    ReportTrends, Trend, compute_trend,
};
