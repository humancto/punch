//! # punch-desktop
//!
//! Native desktop wrapper for the Punch Agent OS.
//!
//! Provides a desktop binary that starts the Arena server programmatically,
//! opens a browser to the dashboard, and exposes IPC commands for integration
//! with future Tauri/webview frontends.

pub mod app;
pub mod commands;
pub mod ipc;
pub mod state;
