# Contributing to Punch

Thanks for your interest in contributing to Punch. This document covers everything you need to get started.

## Quick Start

```bash
git clone https://github.com/humancto/punch.git
cd punch
cargo build
cargo test --workspace
```

## Development Setup

### Requirements

- Rust 2024 edition (1.85+)
- SQLite development headers (usually bundled via `rusqlite`)
- For testing with local LLMs: [Ollama](https://ollama.com)

### Build Commands

```bash
cargo build                                # Debug build
cargo build --release                      # Release build (optimized, LTO, stripped)
cargo test --workspace                     # Run all tests
cargo test -p punch-kernel                 # Test a specific crate
cargo clippy --workspace -- -D warnings    # Lint (must pass with zero warnings)
cargo fmt --all                            # Format code
cargo fmt --all -- --check                 # Check formatting without modifying
```

All three checks (test, clippy, fmt) must pass before submitting a PR.

## Workspace Structure

Punch is a Rust workspace of 12 crates. Dependencies flow strictly downward — no circular dependencies.

```
punch-types          (foundation — all crates depend on this)
  +-- punch-memory   (SQLite, memory decay, creeds)
  +-- punch-skills   (tool registry, marketplace, MCP)
  +-- punch-extensions (WASM plugin system)
  +-- punch-wire     (P2P protocol, HMAC-SHA256 auth)
        +-- punch-runtime (fighter loop, LLM drivers, tool executor)
              +-- punch-kernel (The Ring — central coordinator)
                    +-- punch-api (The Arena — HTTP/WS API)
                    +-- punch-channels (26 platform adapters)
                    +-- punch-gorillas (autonomous agent system)
                          +-- punch-cli (binary entry point)
```

**Rule:** `punch-types` is the only crate every other crate may depend on.

## Conventions

### Combat Metaphor

Punch uses a combat metaphor throughout. Use the terminology consistently:

| Term      | Meaning                     |
| --------- | --------------------------- |
| Fighter   | Interactive AI agent        |
| Gorilla   | Autonomous background agent |
| Move      | A skill or tool             |
| The Ring  | Central kernel/coordinator  |
| The Arena | HTTP API server             |
| Bout      | Conversation session        |
| Creed     | Agent identity document     |
| Troop     | Coordinated agent squad     |
| Spawn     | Create a fighter            |
| Kill      | Terminate a fighter         |
| Unleash   | Start a gorilla             |
| Cage      | Stop a gorilla              |

### Code Style

- **Error handling:** Use `PunchResult<T>` and `thiserror`. Never `.unwrap()` or `.expect()` in library crates.
- **Logging:** Use `tracing` with structured fields: `info!(%id, name, "fighter spawned")`
- **Concurrency:** Use `DashMap` for concurrent collections, `tokio::sync::Mutex` for async locking.
- **Naming:** `PascalCase` for types, `snake_case` for modules, `SCREAMING_SNAKE_CASE` for constants.
- **Serialization:** `serde` for all public types. TOML for config, JSON for API.

### Testing

- Every public function needs at least one test
- Use `#[tokio::test]` for async tests
- Mock `LlmDriver` in tests — never make real API calls
- Test both happy paths and error conditions

## Branch Naming

```
feat/add-fighter-templates
fix/gorilla-schedule-parsing
refactor/ring-event-bus
docs/getting-started-guide
test/troop-consensus-strategy
```

## Commit Messages

Use imperative mood. Reference the combat metaphor where applicable.

```
Add Pipeline troop coordination strategy
Fix gorilla schedule parsing for sub-minute intervals
Refactor Ring event bus to use broadcast channel
```

## Pull Request Process

1. Fork the repo and create a feature branch
2. Make your changes with tests
3. Ensure all checks pass: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`
4. Submit a PR with a clear description of what and why
5. Wait for review — we aim to review within 48 hours

### PR Description Template

Your PR description should include:

- **What** changed and **why**
- Any breaking changes with migration notes
- New dependencies must be justified
- Security-related changes require two reviewer approvals

## Adding Dependencies

Prefer dependencies already in `workspace.dependencies`. If you need a new dependency:

1. Add it to the workspace `Cargo.toml` first
2. Reference it in the crate's `Cargo.toml` with `workspace = true`
3. Justify the addition in your PR description

## Security

- Changes to security layers require review from at least two maintainers
- Gorilla manifest changes need special review for prompt injection risks
- Never commit secrets, API keys, or credentials
- Report vulnerabilities to `security@humancto.com` — do not open public issues

## Community

- [GitHub Discussions](https://github.com/humancto/punch/discussions) — Questions, ideas, show & tell
- [GitHub Issues](https://github.com/humancto/punch/issues) — Bug reports, feature requests

## License

By contributing to Punch, you agree that your contributions will be licensed under the MIT License.
