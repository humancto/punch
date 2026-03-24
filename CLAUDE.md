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
  └─► punch-wire     (depends on punch-types — P2P protocol, HMAC-SHA256 auth)
        └─► punch-runtime (depends on punch-wire, punch-memory, punch-extensions, punch-types — fighter loop, LLM driver)
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

## The Creed System

Creeds are database-backed agent identity documents that persist across respawns and evolve with every interaction.

### Lifecycle

1. **Auto-creation on spawn** — `Ring::spawn_fighter()` creates a default Creed with self-awareness (model, provider, weight class) if one doesn't exist. The creed is bound to the fighter automatically.
2. **Injection** — `creed.render()` is injected into every LLM system prompt during a bout.
3. **Evolution** — After each bout: `bout_count++`, relationships updated, learned behaviors reinforced or decayed.
4. **Heartbeat tasks** — Proactive tasks with cadences (`every_bout`, `on_wake`, `hourly`, `daily`) are injected as due items in the system prompt. After a bout completes, due tasks are marked as checked with updated `execution_count`.
5. **Persistence** — Creeds survive fighter kills and respawns via SQLite storage.

### Key types

- `punch-types::creed::Creed` — Core identity document with traits, directives, relationships, heartbeat tasks
- `punch-memory` — `save_creed()`, `load_creed_by_name()`, `bind_creed_to_fighter()`

## LLM Streaming

All 6 LLM drivers implement real streaming via `stream_complete_with_callback()`:

- **AnthropicDriver** — SSE with `"stream": true`, parses `message_start`, `content_block_delta`, `message_delta` events
- **OpenAiCompatibleDriver** — SSE with `"stream": true`, parses `choices[0].delta`, handles `[DONE]`
- **GeminiDriver** — SSE via `streamGenerateContent?alt=sse` endpoint
- **OllamaDriver** — Newline-delimited JSON with `"stream": true`, `"done": true` terminator
- **BedrockDriver** — Falls back to non-streaming (Bedrock uses proprietary binary event framing)
- **AzureOpenAiDriver** — Delegates to OpenAI-compatible SSE parser with Azure URL/headers

Key types: `StreamChunk`, `ToolCallDelta`, `StreamCallback` (all in `punch-runtime::driver`)

## Tool Capabilities

### WASM Plugin Invocation (`wasm_invoke`)

- Tool: `wasm_invoke` — Invokes a WASM plugin by name with JSON input
- Capability gate: `Capability::PluginInvoke`
- Uses `punch-extensions::PluginRegistry` for plugin lookup and execution
- Runtime: **Wasmtime 29** (JIT compiler) with dual metering: fuel-based instruction limits + epoch-based wall-clock interruption
- Located in `punch-runtime::tool_executor`

### A2A Outbound Delegation (`a2a_delegate`)

- Tool: `a2a_delegate` — Delegates a task to a remote agent via Google's A2A protocol
- Capability gate: `Capability::A2ADelegate`
- Discovers remote agent, sends task, polls for completion with timeout
- Located in `punch-runtime::tool_executor`

## Troop Coordination

Troops provide multi-agent task coordination with 6 strategies, all with real result collection:

- **LeaderWorker** — Leader decomposes task, fans out subtasks to workers, collects results
- **RoundRobin** — Routes to next member in rotation, collects response
- **Broadcast** — Sends to all members concurrently, collects all responses
- **Pipeline** — Chains stages sequentially — output of stage N becomes input of stage N+1
- **Consensus** — Sends vote requests to all, tallies responses, returns majority decision
- **Specialist** — Routes to best capability match based on task keywords

All strategies use `MessageRouter::request()` with configurable timeout (default 60s). API endpoint returns `assigned_to`, `routing_decision`, and full `results` array.

## Combat Theme Terminology Reference

| Term        | Meaning                              | Code Location                                    |
| ----------- | ------------------------------------ | ------------------------------------------------ |
| Fighter     | Interactive/conversational AI agent  | `punch-types::fighter`, `punch-runtime`          |
| Gorilla     | Autonomous background AI agent       | `punch-types::gorilla`, `punch-gorillas`         |
| Move        | A skill or tool an agent can use     | `punch-skills`, `punch-types::tool`              |
| The Ring    | Central execution kernel/coordinator | `punch-kernel::ring::Ring`                       |
| The Arena   | HTTP API server                      | `punch-api`                                      |
| Bout        | A conversation session with memory   | `punch-memory::BoutId`                           |
| Creed       | Database-backed agent identity       | `punch-types::creed::Creed`                      |
| Heartbeat   | Proactive task on a cadence          | `punch-types::creed::HeartbeatTask`              |
| Combo       | Chained multi-agent workflow         | `punch-runtime` (planned)                        |
| Troop       | Coordinated group of agents          | `punch-kernel::troop::TroopManager`              |
| Spawn       | Create a new fighter                 | `Ring::spawn_fighter()`                          |
| Kill        | Terminate a fighter                  | `Ring::kill_fighter()`                           |
| Unleash     | Start a gorilla                      | `Ring::unleash_gorilla()`                        |
| Cage        | Stop a gorilla                       | `Ring::cage_gorilla()`                           |
| Rampaging   | Gorilla actively executing           | `GorillaStatus::Rampaging`                       |
| KnockedOut  | Fighter that errored out             | `FighterStatus::KnockedOut`                      |
| Resting     | Fighter that's rate-limited          | `FighterStatus::Resting`                         |
| WeightClass | Fighter capability tier              | `punch-types::fighter::WeightClass`              |
| A2A         | Agent-to-Agent protocol delegation   | `punch-runtime::tool_executor`                   |
| Channel     | Messaging platform adapter           | `punch-channels`, `punch-cli::commands::channel` |

## Channel System

Channels connect fighters to external messaging platforms (Telegram, Slack, Discord, etc.) via webhooks.

### Setup Wizard

`punch channel setup <platform>` — interactive wizard that handles bot creation guidance, credential collection, webhook secret generation, tunnel setup, and webhook registration.

### Architecture

- **One tunnel, many channels** — a single public URL (Cloudflare Tunnel or BYO) is saved to `[tunnel]` in `~/.punch/config.toml` and shared across all channels
- **Three tunnel modes**: quick (temporary dev), named (persistent Cloudflare), manual (BYO URL)
- **Config storage**: Channel configs in `~/.punch/config.toml`, secrets in `~/.punch/.env` (env var references, never plaintext tokens in config)
- **Security layers**: Signature verification (platform-specific HMAC/Ed25519), user allowlist, per-user rate limiting, auth isolation per channel

### Key types

- `punch-types::config::TunnelConfig` — base_url + mode (quick/named/manual)
- `punch-types::config::ChannelConfig` — channel_type, token_env, webhook_secret_env, allowed_user_ids, rate_limit_per_user
- `punch-channels::onboarding::OnboardingGuide` — step-by-step setup instructions per platform
- `punch-channels::router::ChannelRouter` — persistent message router surviving across webhook requests
- `punch-channels::security::SecurityGateway` — signature verification + allowlist + rate limiting

### CLI commands

| Command                           | What it does                         |
| --------------------------------- | ------------------------------------ |
| `punch channel setup <platform>`  | Interactive setup wizard             |
| `punch channel list`              | Show configured channels             |
| `punch channel tunnel`            | Show/update/remove shared tunnel URL |
| `punch channel remove <platform>` | Remove channel config + secrets      |
| `punch channel test <platform>`   | Send test payload to webhook         |
| `punch channel status <name>`     | Query daemon for live channel status |

### Documentation

- `docs/channels.md` — Full channel guide (setup, security, tunnel modes, management, troubleshooting)
- `docs/getting-started.md` Step 10 — Points to the wizard

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
│   ├── channels.md             # Channel setup, tunnel modes, management
│   ├── getting-started.md      # Quickstart guide (10 steps)
│   └── security.md             # Security documentation
└── crates/
    ├── punch-cli/              # Binary crate (main entry point)
    │   └── src/
    │       ├── main.rs
    │       └── cli.rs          # Clap definitions
    ├── punch-types/            # Shared types
    │   └── src/
    │       ├── lib.rs          # Re-exports
    │       ├── capability.rs   # Includes PluginInvoke, A2ADelegate
    │       ├── config.rs
    │       ├── creed.rs        # Creed identity, heartbeat tasks
    │       ├── error.rs
    │       ├── event.rs
    │       ├── fighter.rs
    │       ├── gorilla.rs
    │       ├── message.rs
    │       └── tool.rs         # Includes Plugin tool category
    ├── punch-memory/           # Memory substrate
    ├── punch-kernel/           # The Ring
    │   └── src/
    │       ├── ring.rs         # Central coordinator (auto-creed on spawn)
    │       ├── background.rs   # Gorilla scheduler (cron + human-readable parsing)
    │       ├── troop.rs        # TroopManager, 6 coordination strategies
    │       ├── agent_messaging.rs # MessageRouter (direct, broadcast, multicast, request-response)
    │       ├── event_bus.rs
    │       └── scheduler.rs
    ├── punch-runtime/          # Fighter loop, LLM drivers (15 providers, streaming), tool_executor
    ├── punch-api/              # The Arena (HTTP API)
    ├── punch-channels/         # Channel adapters
    ├── punch-skills/           # Moves (tools)
    ├── punch-gorillas/         # Gorilla system
    │   └── bundled/
    │       └── alpha/
    │           └── GORILLA.toml
    ├── punch-extensions/       # WASM plugin system (Wasmtime 29, dual metering)
    ├── punch-wire/             # P2P protocol, HMAC-SHA256 auth
    └── xtask/                  # Build automation
```
