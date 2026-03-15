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

        /// Model to use for the chat (overrides config)
        #[arg(short, long)]
        model: Option<String>,

        /// System prompt for the chat
        #[arg(short, long)]
        system: Option<String>,

        /// Enable streaming output
        #[arg(long)]
        stream: bool,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Manage multi-step agent workflows
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
    },

    /// Manage channel adapters (Telegram, Discord, Slack)
    Channel {
        #[command(subcommand)]
        command: ChannelCommands,
    },

    /// Manage event-driven triggers
    Trigger {
        #[command(subcommand)]
        command: TriggerCommands,
    },

    /// Launch the interactive ringside monitor (TUI dashboard)
    Tui,

    /// Open the Punch dashboard in a browser or native webview
    Desktop {
        /// Port for the Arena server (overrides config)
        #[arg(short, long)]
        port: Option<u16>,

        /// Launch in native webview mode (requires `desktop` feature)
        #[arg(long)]
        native: bool,
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

        /// Fighter name override
        #[arg(short, long)]
        name: Option<String>,

        /// Model to use (overrides template default)
        #[arg(short, long)]
        model: Option<String>,
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
        #[arg(short, long)]
        message: String,
    },

    /// Terminate a fighter
    Kill {
        /// Fighter ID
        id: String,
    },

    /// Show fighter status
    Status {
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

    /// Run a single autonomous tick of a gorilla (for testing)
    Test {
        /// Gorilla name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MoveCommands {
    /// List installed moves/skills
    List,

    /// Search for moves (local and marketplace)
    Search {
        /// Search query
        query: String,
    },

    /// Show detailed info about a move
    Info {
        /// Move name
        name: String,
    },

    /// Install a move from the marketplace
    Install {
        /// Move name
        name: String,

        /// Specific version to install (default: latest)
        #[arg(short, long)]
        version: Option<String>,
    },

    /// Publish a skill to the marketplace
    Publish {
        /// Path to the skill directory
        #[arg(default_value = ".")]
        dir: String,

        /// Perform a dry run (validate and scan without publishing)
        #[arg(long)]
        dry_run: bool,
    },

    /// Update installed marketplace moves
    Update {
        /// Specific move to update (updates all if omitted)
        name: Option<String>,
    },

    /// Remove an installed marketplace move
    Remove {
        /// Move name
        name: String,
    },

    /// Generate an Ed25519 keypair for signing skills
    Keygen,

    /// Report a skill for abuse or security issues
    Report {
        /// Move name
        name: String,

        /// Reason for the report
        #[arg(short, long)]
        reason: String,
    },

    /// Verify a skill's signature and integrity
    Verify {
        /// Move name
        name: String,
    },

    /// Run a security scan on a skill
    Scan {
        /// Path to a SKILL.md file or skill directory
        path: String,
    },

    /// Sync the marketplace index
    Sync,

    /// Show or update the lock file
    Lock,
}

#[derive(Debug, Subcommand)]
pub enum WorkflowCommands {
    /// List registered workflows
    List,

    /// Execute a workflow
    Run {
        /// Workflow name or ID
        #[arg(value_name = "NAME")]
        id: String,
        /// Input text for the workflow
        #[arg(short, long)]
        input: String,
    },

    /// Check the status of a workflow run
    Status {
        /// Run ID
        run_id: String,
    },

    /// Create a workflow from a definition file
    Create {
        /// Path to workflow definition file (TOML or JSON)
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ChannelCommands {
    /// List configured channels and their status
    List,

    /// Send a test message through a channel adapter
    Test {
        /// Platform to test (telegram, discord, slack)
        platform: String,
    },

    /// Show detailed status of a channel
    Status {
        /// Channel name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TriggerCommands {
    /// List all registered triggers
    List,

    /// Add a new trigger
    Add {
        /// Trigger type: keyword, schedule, event, webhook
        #[arg(long, name = "type")]
        trigger_type: String,
        /// Gorilla to associate with the trigger
        #[arg(long)]
        gorilla: Option<String>,
        /// Configuration (JSON string)
        #[arg(long)]
        config: String,
    },

    /// Remove a trigger by ID
    Remove {
        /// Trigger ID (UUID)
        id: String,
    },

    /// Test a trigger (dry run)
    Test {
        /// Trigger ID (UUID)
        id: String,
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
