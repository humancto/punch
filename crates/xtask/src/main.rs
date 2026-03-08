//! # xtask
//!
//! Build automation for the Punch Agent Combat System.
//!
//! Run with: `cargo xtask <subcommand>`

use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Build automation for Punch")]
struct Cli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Subcommand)]
enum XtaskCommand {
    /// Build all crates in release mode.
    BuildRelease,
    /// Run all tests across the workspace.
    TestAll,
    /// Run clippy lints on the workspace.
    Lint,
    /// Format all code with rustfmt.
    Fmt,
    /// Run the full CI pipeline (fmt check, lint, test).
    Ci,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        XtaskCommand::BuildRelease => build_release(),
        XtaskCommand::TestAll => test_all(),
        XtaskCommand::Lint => lint(),
        XtaskCommand::Fmt => fmt(),
        XtaskCommand::Ci => ci(),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_cargo(args: &[&str]) -> Result<(), String> {
    println!(">> cargo {}", args.join(" "));
    let status = Command::new("cargo")
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo {} failed with exit code {}",
            args.join(" "),
            status.code().unwrap_or(-1)
        ))
    }
}

fn build_release() -> Result<(), String> {
    run_cargo(&["build", "--workspace", "--release"])
}

fn test_all() -> Result<(), String> {
    run_cargo(&["test", "--workspace"])
}

fn lint() -> Result<(), String> {
    run_cargo(&["clippy", "--workspace", "--", "-D", "warnings"])
}

fn fmt() -> Result<(), String> {
    run_cargo(&["fmt", "--all"])
}

fn ci() -> Result<(), String> {
    println!("=== CI: Format Check ===");
    run_cargo(&["fmt", "--all", "--check"])?;

    println!("\n=== CI: Lint ===");
    lint()?;

    println!("\n=== CI: Test ===");
    test_all()?;

    println!("\n=== CI: All checks passed ===");
    Ok(())
}
