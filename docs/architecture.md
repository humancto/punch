# Punch Architecture

> Deep-dive into the internal architecture of the Punch Agent Combat System.

## Overview

Punch is a Rust workspace of 12 crates that compile into a single binary. The architecture follows a layered design where each crate has a single responsibility, strict dependency boundaries, and clear ownership of its domain.

## Crate Dependency Graph

```
                           ┌──────────────┐
                           │  punch-cli   │  (binary entry point)
                           └──────┬───────┘
                                  │
                    ┌─────────────┼─────────────┐
                    │             │             │
             ┌──────▼──────┐ ┌───▼────────┐ ┌──▼────────────┐
             │  punch-api  │ │punch-kernel│ │punch-channels │
             │ (The Arena) │ │ (The Ring) │ │  (Adapters)   │
             └──────┬──────┘ └───┬────┬───┘ └──────┬────────┘
                    │            │    │             │
                    └────────┬───┘    │      ┌─────┘
                             │        │      │
                    ┌────────▼──────┐ │  ┌───▼────────────┐
                    │ punch-runtime │ │  │ punch-gorillas │
                    │ (Fighter loop)│ │  │ (Manifests)    │
                    └───┬──────┬───┘ │  └───┬────────────┘
                        │      │     │      │
              ┌─────────┘      │     │      │
              │         ┌──────┘     │      │
              │         │            │      │
        ┌─────▼───┐ ┌───▼──────┐ ┌──▼──────▼──┐ ┌────────────────┐
        │punch-wire│ │punch-    │ │punch-skills│ │punch-extensions│
        │(LLM SDK) │ │ memory   │ │  (Moves)   │ │  (Plugins)     │
        └─────┬───┘ └────┬─────┘ └──────┬─────┘ └───────┬────────┘
              │          │              │               │
              └──────────┴──────────────┴───────────────┘
                                  │
                         ┌────────▼────────┐
                         │   punch-types   │
                         │ (Foundation)    │
                         └─────────────────┘
```

### Dependency Rules

1. **punch-types** is the foundation — all crates depend on it, it depends on nothing internal
2. **Leaf crates** (punch-wire, punch-memory, punch-skills, punch-extensions) depend only on punch-types
3. **punch-runtime** aggregates leaf crates to implement the fighter execution loop
4. **punch-kernel** (The Ring) depends on punch-runtime and punch-memory
5. **punch-cli** sits at the top, wiring everything together
6. **No circular dependencies** — enforced by Cargo's resolver

## Data Flow: User Input to Response

This is the complete path a user message takes through the system:

```
 User types: "What is quantum computing?"
      │
      ▼
 ┌─────────────────────────────────┐
 │ 1. CLI / Channel / Arena API    │  Entry points parse the request
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 2. Ring.send_message()          │  The Ring looks up the fighter,
 │    - Validate fighter exists    │  checks quotas, ensures a bout
 │    - Check scheduler quota      │  exists, then delegates to the
 │    - Get or create bout         │  runtime.
 │    - Set status → Fighting      │
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 3. run_fighter_loop()           │  The runtime's agent loop:
 │    - Load bout history from     │  a) Fetches conversation context
 │      MemorySubstrate            │  b) Builds the LLM prompt
 │    - Build prompt with system   │  c) Calls the LLM
 │      message + context          │  d) Processes tool calls
 │    - Call LlmDriver.complete()  │  e) Loops until done or max
 │    - Execute tool calls (Moves) │     iterations reached
 │    - Store messages in memory   │
 │    - Repeat if tool calls exist │
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 4. LlmDriver (punch-wire)      │  Translates to provider-specific
 │    - Route to correct provider  │  API format (Anthropic, OpenAI,
 │    - Handle auth, retries       │  etc.), manages streaming, and
 │    - Stream or batch response   │  returns structured response.
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 5. Tool Execution (punch-skills)│  If the LLM requested tool calls:
 │    - Validate capability grants │  a) Check the fighter has the
 │    - Execute move               │     required capability
 │    - Return ToolResult          │  b) Execute the tool
 │    - Feed back into loop (→ 3)  │  c) Return result to the loop
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 6. Memory Persistence           │  All messages (user, assistant,
 │    - Store in SQLite via bout   │  tool calls, results) are persisted
 │    - Apply compaction if needed │  to the memory substrate under
 │    - Update decay scores        │  the bout ID.
 └──────────────┬──────────────────┘
                │
                ▼
 ┌─────────────────────────────────┐
 │ 7. Ring post-processing         │  Ring updates fighter status
 │    - Status → Idle              │  back to Idle, records usage
 │    - Record usage with scheduler│  for quota tracking, and
 │    - Publish event              │  publishes completion event.
 └──────────────┬──────────────────┘
                │
                ▼
 Response returned to user
```

## Gorilla Lifecycle

Gorillas follow a distinct lifecycle from fighters:

```
                    ┌───────────────┐
                    │   GORILLA.toml │  Manifest defines name,
                    │   (manifest)   │  schedule, required moves,
                    └───────┬───────┘  settings, and system prompt
                            │
                            ▼
                    ┌───────────────┐
           ┌───────│    CAGED      │  Registered but not running.
           │       │   (default)    │  Loaded into The Ring's
           │       └───────┬───────┘  gorilla registry.
           │               │
           │     punch gorilla unleash <name>
           │               │
           │               ▼
           │       ┌───────────────┐
           │       │   UNLEASHED   │  Background tokio task spawned.
           │       │  (scheduled)   │  Cron expression determines
           │       └───────┬───────┘  when execution cycles fire.
           │               │
           │         Cron fires
           │               │
           │               ▼
           │       ┌───────────────┐
           │       │   RAMPAGING   │  Actively executing a task cycle.
           │       │  (executing)   │  Uses its assigned moves, writes
           │       └───────┬───────┘  to memory, produces output.
           │               │
           │          Cycle complete
           │               │
           │               ▼
           │       Back to UNLEASHED (waits for next cron fire)
           │
           │     punch gorilla cage <name>
           │               │
           └───────────────┘
                   │
                   ▼
           Task handle aborted,
           status → CAGED
```

### Gorilla Manifest Format (GORILLA.toml)

Each gorilla is defined by a TOML manifest containing:

- **name** — Display name
- **description** — What the gorilla does
- **schedule** — Cron expression for execution timing
- **moves_required** — List of moves (tools) the gorilla needs
- **settings** — Typed configuration parameters with defaults
- **dashboard_metrics** — Metrics the gorilla tracks
- **system_prompt** — The gorilla's personality, methodology, and instructions

## The Ring — Kernel Architecture

The Ring is the central coordinator. It owns all state and enforces all invariants.

```
┌─────────────────────────────────────────────────────┐
│                    THE RING                          │
│                                                     │
│  ┌──────────────┐  ┌──────────────┐                │
│  │   Fighters    │  │   Gorillas   │                │
│  │  DashMap<     │  │  DashMap<    │                │
│  │   FighterId,  │  │   GorillaId, │                │
│  │   Entry>      │  │   Mutex<     │                │
│  │               │  │    Entry>>   │                │
│  └──────────────┘  └──────────────┘                │
│                                                     │
│  ┌──────────────┐  ┌──────────────┐                │
│  │  Event Bus    │  │  Scheduler   │                │
│  │  (pub/sub)    │  │  (quotas,    │                │
│  │               │  │   rate limit)│                │
│  └──────────────┘  └──────────────┘                │
│                                                     │
│  ┌──────────────┐  ┌──────────────┐                │
│  │   Memory      │  │  LLM Driver  │                │
│  │  Substrate    │  │  (Arc<dyn>)  │                │
│  │  (Arc)        │  │              │                │
│  └──────────────┘  └──────────────┘                │
│                                                     │
└─────────────────────────────────────────────────────┘
```

**Thread Safety:** The Ring is `Send + Sync` (compile-time asserted). Fighters use `DashMap` for lock-free concurrent reads. Gorillas use `DashMap` + `tokio::sync::Mutex` because their entries contain non-Clone `JoinHandle`s.

**Event Bus:** Internal pub/sub system that broadcasts lifecycle events (`FighterSpawned`, `BoutStarted`, `GorillaUnleashed`, etc.). Channel adapters and the Arena API subscribe to these events for real-time updates.

**Scheduler:** Manages per-fighter quotas (RPM, TPM). When a fighter exceeds its quota, it enters the `Resting` status and receives a `RateLimited` error with a retry-after hint.

## Memory Architecture

```
┌─────────────────────────────────────────┐
│           Memory Substrate              │
│                                         │
│  ┌─────────────────────────────────┐   │
│  │        SQLite Database           │   │
│  │  ┌───────────┐ ┌──────────────┐ │   │
│  │  │   Bouts    │ │  Messages    │ │   │
│  │  │ (sessions) │ │ (per-bout)   │ │   │
│  │  └───────────┘ └──────────────┘ │   │
│  │  ┌───────────┐ ┌──────────────┐ │   │
│  │  │  Entities  │ │ Relations    │ │   │
│  │  │ (knowledge │ │ (knowledge   │ │   │
│  │  │  graph)    │ │  graph)      │ │   │
│  │  └───────────┘ └──────────────┘ │   │
│  └─────────────────────────────────┘   │
│                                         │
│  ┌─────────────────────────────────┐   │
│  │         Decay Engine             │   │
│  │  - Exponential decay on scores   │   │
│  │  - Configurable decay_rate       │   │
│  │  - Old memories fade over time   │   │
│  └─────────────────────────────────┘   │
│                                         │
│  ┌─────────────────────────────────┐   │
│  │       Compaction Engine          │   │
│  │  - Triggers at threshold %       │   │
│  │  - Keeps N most recent messages  │   │
│  │  - Summarize or truncate strategy│   │
│  └─────────────────────────────────┘   │
│                                         │
│  ┌─────────────────────────────────┐   │
│  │     Encryption Layer (AES-256)   │   │
│  │  - Data encrypted at rest        │   │
│  │  - Master key via Argon2id KDF   │   │
│  │  - Secrets zeroized on drop      │   │
│  └─────────────────────────────────┘   │
│                                         │
└─────────────────────────────────────────┘
```

### Memory Decay

Memories have a relevance score that decays exponentially over time:

```
score(t) = score(0) * e^(-decay_rate * t)
```

Where `t` is time elapsed since the memory was created. This ensures old, unused memories naturally fade while frequently-accessed memories maintain high scores through reinforcement.

### Compaction

When a bout's context exceeds the configured threshold percentage of the model's context window:

1. Messages older than `keep_recent` are candidates for compaction
2. **Summarize strategy:** An LLM call generates a concise summary of the compacted messages
3. **Truncate strategy:** Messages are simply removed (faster, less accurate)
4. The summary replaces the compacted messages in the context

## Security Architecture

Punch implements 18 security layers. See [security.md](security.md) for the complete security documentation.

Key architectural security decisions:

- **Capability-based access:** Every move requires an explicit `CapabilityGrant`. Fighters and gorillas only get access to moves they're explicitly granted.
- **Zeroize on drop:** All cryptographic material (`ed25519-dalek` keys, AES keys, Argon2 derived keys) implements `Zeroize` and is cleared from memory when dropped.
- **No secrets in config:** API keys are always referenced via environment variable names (`_env` suffix pattern), never stored directly in configuration files.
- **Gorilla isolation:** Each gorilla runs in its own containment zone with an independent capability boundary.

## The Arena — HTTP API Architecture

```
┌────────────────────────────────────────────────────┐
│                   THE ARENA                         │
│                  (punch-api)                        │
│                                                     │
│  ┌──────────────────────────────────────────────┐  │
│  │              Axum Router                      │  │
│  │                                               │  │
│  │  POST /api/v1/fighters          (spawn)       │  │
│  │  GET  /api/v1/fighters          (list)        │  │
│  │  POST /api/v1/fighters/:id/chat (send msg)    │  │
│  │  DELETE /api/v1/fighters/:id    (kill)        │  │
│  │                                               │  │
│  │  GET  /api/v1/gorillas          (list)        │  │
│  │  POST /api/v1/gorillas/:id/unleash (start)    │  │
│  │  POST /api/v1/gorillas/:id/cage    (stop)     │  │
│  │                                               │  │
│  │  GET  /api/v1/moves             (list tools)  │  │
│  │  GET  /api/v1/health            (health check)│  │
│  │                                               │  │
│  │  WS   /api/v1/ws                (streaming)   │  │
│  └──────────────────────────────────────────────┘  │
│                                                     │
│  ┌──────────────┐  ┌──────────────────────────┐   │
│  │  Tower       │  │  Middleware Stack          │   │
│  │  Service     │  │  - CORS validation         │   │
│  │  Layer       │  │  - Request tracing         │   │
│  │              │  │  - Gzip compression         │   │
│  │              │  │  - Auth (Ed25519 signing)   │   │
│  │              │  │  - Rate limiting             │   │
│  └──────────────┘  └──────────────────────────┘   │
│                                                     │
└────────────────────────────────────────────────────┘
```

The Arena is built on Axum with Tower middleware for cross-cutting concerns. WebSocket support enables real-time streaming of agent responses. All endpoints require authentication via Ed25519-signed requests.
