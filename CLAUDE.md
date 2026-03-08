# CLAUDE.md — Development Guide for AI Assistants

## Project Overview

Punch is an agent operating system written in Rust (2024 edition). It provides a single-binary platform for deploying interactive AI agents ("fighters") and autonomous background agents ("gorillas"), coordinated through a central kernel ("the Ring") and exposed via an HTTP API ("the Arena").

**Repository:** `https://github.com/humancto/punch`
**License:** MIT
**Authors:** HumanCTO (`team@humancto.com`)

## Build Commands

```bash
# Build all workspace crates
cargo build

# Build release binary (optimized, LTO, stripped)
cargo build --release

# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p punch-kernel

# Lint (must pass with zero warnings)
cargo clippy --workspace -- -D warnings

# Format (must be clean)
cargo fmt --all

# Check formatting without modifying
cargo fmt --all -- --check

# Run the binary
cargo run -- <command>

# Build tasks via xtask
cargo xtask <task>
```

## Workspace Structure & Dependency Order

Crates must be built in dependency order. The dependency graph (leaves first):

```
punch-types          (no internal deps — foundational types, errors, config)
  └─► punch-memory   (depends on punch-types — SQLite, memory decay)
  └─► punch-skills   (depends on punch-types — tool registry, MCP)
  └─► punch-extensions (depends on punch-types — plugin system)
  └─► punch-wire     (depends on punch-types — LLM provider abstraction)
        └─► punch-runtime (depends on punch-wire, punch-memory, punch-types — fighter loop, LLM driver)
              └─► punch-kernel (depends on punch-runtime, punch-memory, punch-types — The Ring)
                    └─► punch-api (depends on punch-kernel — The Arena, Axum HTTP/WS)
                    └─► punch-channels (depends on punch-kernel — Telegram, Discord, etc.)
                    └─► punch-gorillas (depends on punch-types — gorilla manifest loader)
                          └─► punch-cli (depends on punch-kernel, punch-api, punch-gorillas — binary entry point)
```

**Rule:** `punch-types` is the only crate that every other crate may depend on. Never introduce circular dependencies.

## Key Conventions

### Error Handling

- Use `thiserror` for defining error types in `punch-types`
- All crate-internal functions return `PunchResult<T>` (alias for `Result<T, PunchError>`)
- Use `anyhow` only in the CLI crate (`punch-cli`) for top-level error reporting
- Never use `.unwrap()` or `.expect()` in library crates — always propagate errors

### Logging & Tracing

- Use `tracing` (not `log`) for all instrumentation
- Add `#[instrument]` to public async functions with meaningful field annotations
- Use structured fields: `info!(%id, name, "fighter spawned")` not `info!("fighter {} spawned: {}", id, name)`
- Log levels: `error!` for unrecoverable issues, `warn!` for recoverable problems, `info!` for lifecycle events, `debug!` for internal state, `trace!` for hot-path details

### Naming Conventions

- Use the combat metaphor terminology consistently (see Terminology below)
- Struct names: `PascalCase` — `FighterManifest`, `GorillaEntry`, `BoutId`
- Module names: `snake_case` — `event_bus`, `fighter_loop`
- Constants: `SCREAMING_SNAKE_CASE`
- Type aliases for IDs: `FighterId`, `GorillaId`, `BoutId` (newtype wrappers around UUID)

### Concurrency

- The Ring is `Send + Sync` (compile-time asserted)
- Use `DashMap` for concurrent collections (not `Arc<Mutex<HashMap>>`)
- Use `tokio::sync::Mutex` for async-aware locking (not `std::sync::Mutex` in async contexts)
- Always drop `DashMap` guards before `.await` points

### Serialization

- All public types derive `Serialize, Deserialize` via serde
- Configuration files use TOML format
- API payloads use JSON
- Gorilla manifests are TOML files (`GORILLA.toml`)

## Combat Theme Terminology Reference

| Term        | Meaning                              | Code Location                            |
| ----------- | ------------------------------------ | ---------------------------------------- |
| Fighter     | Interactive/conversational AI agent  | `punch-types::fighter`, `punch-runtime`  |
| Gorilla     | Autonomous background AI agent       | `punch-types::gorilla`, `punch-gorillas` |
| Move        | A skill or tool an agent can use     | `punch-skills`, `punch-types::tool`      |
| The Ring    | Central execution kernel/coordinator | `punch-kernel::ring::Ring`               |
| The Arena   | HTTP API server                      | `punch-api`                              |
| Bout        | A conversation session with memory   | `punch-memory::BoutId`                   |
| Combo       | Chained multi-agent workflow         | `punch-runtime` (planned)                |
| Troop       | Coordinated group of agents          | `punch-kernel` (planned)                 |
| Spawn       | Create a new fighter                 | `Ring::spawn_fighter()`                  |
| Kill        | Terminate a fighter                  | `Ring::kill_fighter()`                   |
| Unleash     | Start a gorilla                      | `Ring::unleash_gorilla()`                |
| Cage        | Stop a gorilla                       | `Ring::cage_gorilla()`                   |
| Rampaging   | Gorilla actively executing           | `GorillaStatus::Rampaging`               |
| KnockedOut  | Fighter that errored out             | `FighterStatus::KnockedOut`              |
| Resting     | Fighter that's rate-limited          | `FighterStatus::Resting`                 |
| WeightClass | Fighter capability tier              | `punch-types::fighter::WeightClass`      |

## Testing Requirements

- Every public function must have at least one unit test
- Integration tests go in `tests/` directories within each crate
- Use `#[tokio::test]` for async tests
- Mock the `LlmDriver` trait for kernel and runtime tests — never make real API calls in tests
- Test both happy paths and error conditions
- The Ring's `Send + Sync` compile-time assertion must never be removed
- Run `cargo test --workspace` before every commit

## PR Guidelines

1. **Branch naming:** `feat/`, `fix/`, `refactor/`, `docs/`, `test/` prefixes
2. **Commit messages:** Imperative mood, reference the combat metaphor where applicable
3. **Required checks:** `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`
4. **Breaking changes:** Document in the PR description with migration notes
5. **New crate dependencies:** Must be justified in the PR description — prefer the dependencies already in `workspace.dependencies`
6. **Security changes:** Require review from at least two maintainers
7. **Gorilla manifests:** Changes to bundled gorillas need special review for prompt injection risks

## File Structure Reference

```
punch/
├── Cargo.toml                  # Workspace root
├── CLAUDE.md                   # This file
├── README.md                   # Project README
├── punch.toml.example          # Example configuration
├── docs/
│   ├── architecture.md         # Architecture deep-dive
│   └── security.md             # Security documentation
└── crates/
    ├── punch-cli/              # Binary crate (main entry point)
    │   └── src/
    │       ├── main.rs
    │       └── cli.rs          # Clap definitions
    ├── punch-types/            # Shared types
    │   └── src/
    │       ├── lib.rs          # Re-exports
    │       ├── capability.rs
    │       ├── config.rs
    │       ├── error.rs
    │       ├── event.rs
    │       ├── fighter.rs
    │       ├── gorilla.rs
    │       ├── message.rs
    │       └── tool.rs
    ├── punch-memory/           # Memory substrate
    ├── punch-kernel/           # The Ring
    │   └── src/
    │       ├── ring.rs         # Central coordinator
    │       ├── event_bus.rs
    │       └── scheduler.rs
    ├── punch-runtime/          # Fighter execution loop
    ├── punch-api/              # The Arena (HTTP API)
    ├── punch-channels/         # Channel adapters
    ├── punch-skills/           # Moves (tools)
    ├── punch-gorillas/         # Gorilla system
    │   └── bundled/
    │       └── alpha/
    │           └── GORILLA.toml
    ├── punch-extensions/       # Plugin system
    ├── punch-wire/             # LLM provider abstraction
    └── xtask/                  # Build automation
```
