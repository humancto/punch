//! `punch move` — Manage moves (skills/tools).

use super::punch_home;
use crate::cli::MoveCommands;

pub async fn run(command: MoveCommands) -> i32 {
    match command {
        MoveCommands::List => run_list().await,
        MoveCommands::Search { query } => run_search(query).await,
        MoveCommands::Install { name } => run_install(name).await,
    }
}

async fn run_list() -> i32 {
    let moves_dir = punch_home().join("moves");

    println!();
    println!("  INSTALLED MOVES");
    println!("  ===============");
    println!();

    if !moves_dir.exists() {
        println!("  No moves installed.");
        println!("  Search for moves: punch move search <query>");
        println!();
        return 0;
    }

    let entries: Vec<_> = match std::fs::read_dir(&moves_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect(),
        Err(e) => {
            eprintln!("  [X] Failed to read moves directory: {}", e);
            return 1;
        }
    };

    if entries.is_empty() {
        println!("  No moves installed.");
        println!("  Search for moves: punch move search <query>");
        println!();
        return 0;
    }

    println!("  {:<24}  FILE", "NAME");
    println!("  {}", "-".repeat(60));

    for entry in &entries {
        let name = entry
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        println!("  {:<24}  {}", name, entry.path().display());
    }

    println!();
    println!("  Total: {} move(s)", entries.len());
    println!();

    0
}

async fn run_search(query: String) -> i32 {
    println!();
    println!("  Searching for moves matching \"{}\"...", query);
    println!();
    println!("  The moves registry is not yet available.");
    println!("  Check https://punch.sh/moves for available moves.");
    println!();

    0
}

async fn run_install(name: String) -> i32 {
    println!();
    println!("  Installing move \"{}\"...", name);
    println!();
    println!("  The moves registry is not yet available.");
    println!("  To install a move manually, place a .toml definition in:");
    println!("    {}", punch_home().join("moves").display());
    println!();

    0
}
