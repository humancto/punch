//! Clap CLI definitions for the `punch` binary.

use clap::{Parser, Subcommand};

/// Punch — The Agent Combat System.
///
/// Deploy autonomous AI agent squads from a single binary.
#[derive(Debug, Parser)]
#[command(
    name = "punch",
    version,
    about = "Punch — The Agent Combat System",
    long_about = "Deploy autonomous AI agent squads from a single binary.\n\nFighters are conversational agents. Gorillas are autonomous background agents.\nThe Ring coordinates them all. The Arena exposes the HTTP API.",
    after_help = "Run `punch <command> --help` for more information on a specific command."
)]
pub struct PunchCli {
    /// Enable verbose logging output.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Path to config file (default: ~/.punch/config.toml).
    #[arg(short, long, global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize Punch (create ~/.punch/, config, etc.)
    Init,

    /// Start the daemon (Ring + Arena)
    Start {
        /// Port to listen on (overrides config)
        #[arg(short, long)]
        port: Option<u16>,
    },

    /// Stop the daemon
    Stop,

    /// Show daemon status
    Status,

    /// Health diagnostics
    Doctor,

    /// Manage fighters (conversational agents)
    Fighter {
        #[command(subcommand)]
        command: FighterCommands,
    },

    /// Manage gorillas (autonomous agents)
    Gorilla {
        #[command(subcommand)]
        command: GorillaCommands,
    },

    /// Manage moves (skills/tools)
    #[command(name = "move")]
    Move {
        #[command(subcommand)]
        command: MoveCommands,
    },

    /// Quick chat with the default fighter
    Chat {
        /// Optional message (interactive mode if omitted)
        message: Option<String>,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Print version information
    Version,
}

#[derive(Debug, Subcommand)]
pub enum FighterCommands {
    /// Create a fighter from a template
    Spawn {
        /// Template name or path
        template: String,
    },

    /// List all fighters
    List,

    /// Interactive chat with a fighter
    Chat {
        /// Fighter name or ID (uses default if omitted)
        name: Option<String>,
    },

    /// Send a one-off message to a fighter
    Send {
        /// Fighter ID
        id: String,
        /// Message to send
        message: String,
    },

    /// Terminate a fighter
    Kill {
        /// Fighter ID
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum GorillaCommands {
    /// List all gorillas
    List,

    /// Activate a gorilla
    Unleash {
        /// Gorilla name
        name: String,
    },

    /// Deactivate a gorilla
    Cage {
        /// Gorilla name
        name: String,
    },

    /// Check gorilla metrics
    Status {
        /// Gorilla name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MoveCommands {
    /// List installed moves/skills
    List,

    /// Search for moves
    Search {
        /// Search query
        query: String,
    },

    /// Install a move
    Install {
        /// Move name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Print current configuration
    Show,

    /// Open config in editor
    Edit,

    /// Set a config value
    Set {
        /// Config key (dot-separated path)
        key: String,
        /// Value to set
        value: String,
    },
}
