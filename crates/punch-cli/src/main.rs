//! Entry point for the `punch` CLI binary.

mod cli;
mod commands;

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use cli::{Commands, PunchCli};

#[tokio::main]
async fn main() {
    let cli = PunchCli::parse();

    // Setup tracing.
    let filter = if cli.verbose {
        EnvFilter::new("punch=debug,tower_http=debug")
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("punch=info,tower_http=info"))
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).with_ansi(true))
        .with(filter)
        .init();

    let exit_code = match cli.command {
        Commands::Init => commands::init::run().await,
        Commands::Start { port } => commands::start::run(cli.config, port).await,
        Commands::Stop => commands::status::run_stop().await,
        Commands::Status => commands::status::run_status().await,
        Commands::Doctor => commands::status::run_doctor().await,
        Commands::Fighter { command } => commands::fighter::run(command, cli.config).await,
        Commands::Gorilla { command } => commands::gorilla::run(command, cli.config).await,
        Commands::Move { command } => commands::moves::run(command).await,
        Commands::Chat { message } => commands::fighter::run_quick_chat(message, cli.config).await,
        Commands::Workflow { command } => commands::workflow::run(command).await,
        Commands::Channel { command } => commands::channel::run(command, cli.config).await,
        Commands::Trigger { command } => commands::trigger::run(command).await,
        Commands::Config { command } => commands::config::run(command).await,
        Commands::Version => {
            println!(
                "punch {} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_HOMEPAGE")
            );
            0
        }
    };

    std::process::exit(exit_code);
}
