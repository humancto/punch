//! Punch Desktop — native desktop entry point for the Punch Agent OS.
//!
//! This binary starts the Arena server in the background, opens a browser
//! to the dashboard, and waits for Ctrl+C to shut down.

use clap::Parser;
use tracing::info;

use punch_desktop::app::{DesktopApp, DesktopConfig};
use punch_desktop::state::Theme;

/// Punch Desktop — The Agent Combat System desktop wrapper.
#[derive(Parser, Debug)]
#[command(
    name = "punch-desktop",
    about = "Punch Desktop — native desktop application"
)]
struct Cli {
    /// Port for the Arena API server.
    #[arg(long, default_value = "6660")]
    port: u16,

    /// Do not open the browser on startup.
    #[arg(long)]
    no_browser: bool,

    /// Theme for the dashboard (dark, light, system).
    #[arg(long, default_value = "dark")]
    theme: String,

    /// API key for Arena authentication.
    #[arg(long, env = "PUNCH_API_KEY")]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let theme: Theme = cli.theme.parse().unwrap_or_else(|_| {
        eprintln!("warning: unknown theme '{}', defaulting to dark", cli.theme);
        Theme::Dark
    });

    let config = DesktopConfig {
        port: cli.port,
        open_browser: !cli.no_browser,
        theme,
        api_key: cli.api_key,
    };

    let mut app = DesktopApp::new(config);
    app.print_banner();

    info!("punch-desktop starting");

    // Try to connect to an already-running Arena.
    if app.wait_for_arena(3).await {
        info!("connected to running arena");
    } else {
        info!(
            port = app.config().port,
            "arena not detected — start the arena separately with: cargo run -- serve"
        );
        println!("  Arena not detected on port {}.", app.config().port);
        println!("  Start the Arena first:  cargo run -- serve");
        println!("  Then restart punch-desktop.");
        println!();
        println!("  Waiting for Arena to come online... (Ctrl+C to quit)");
        println!();

        // Keep trying in the background.
        if !app.wait_for_arena(30).await {
            eprintln!("  Could not connect to Arena after 30 seconds.");
            eprintln!("  Running in disconnected mode. Start the Arena to enable all features.");
            println!();
        }
    }

    // Open the browser if configured and connected.
    if app.state().connected && app.state().auto_open_browser {
        app.open_browser();
    }

    println!("  Press Ctrl+C to quit.");
    println!();

    // Wait for shutdown signal.
    tokio::signal::ctrl_c().await?;

    info!("punch-desktop shutting down");
    println!();
    println!("  Goodbye.");

    Ok(())
}
