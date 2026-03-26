# Changelog

All notable changes to Punch will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] — 2026-03-26

### The Efficiency Update

Token efficiency, budget controls, and cost observability — keeping agents powerful while keeping costs under control.

### Added

#### Token Efficiency Engine

- **Anthropic prompt caching** — `cache_control: ephemeral` on system prompts and tool definitions for 90% cost reduction on cached portions (PR #17)
- **Gemini systemInstruction** — System prompts use the dedicated `systemInstruction` API field for automatic caching (PR #17)
- **Compact creed rendering** — `Creed::render_compact()` omits empty sections and uses terse format, cutting creed tokens from 700-2,500 to 300-800 (PR #17)
- **Dynamic tool selection** — `ToolSelector` with 17 tool groups and keyword-activated on-demand loading. Replaces sending all 70+ tools every turn. Summary fallback for unloaded tools with one-retry auto-activation (PR #18)
- **Tool description compression** — Single-line summaries replace verbose multi-sentence descriptions, cutting ~40% per tool definition (PR #19)
- **Adaptive max_tokens** — Cheap tier: 1024, Mid: 2048, Expensive: 4096. Ollama reasoning models: 16384. Prevents output token waste on simple queries (PR #19)
- **Conditional reflection** — Post-bout reflection LLM call skipped for simple exchanges (<6 messages and no tool use), saving ~80% of reflection costs (PR #19)
- **Sliding window summarization** — After 10+ messages in a bout, early messages are summarized into a ~200 token recap. Keeps context while slashing tokens on long conversations (PR #20)

#### Cost Observability

- **Token usage tracking** — Per-fighter, per-model cost recording in SQLite `usage_events` table with `MeteringEngine` (PR #21)
- **`punch stats` CLI command** — Daily/monthly token spend per fighter, per model with formatted tables (PR #21)
- **Stats API** — `GET /api/stats` (global) and `GET /api/stats/fighters/{id}` with `?period=hour|day|month` query parameter (PR #21)
- **Per-model and per-fighter breakdowns** — `ModelUsageBreakdown` and `FighterUsageBreakdown` types with GROUP BY queries (PR #21)

#### Budget Controls

- **BudgetConfig** — `[budget]` section in config with `daily_cost_limit_usd`, `monthly_cost_limit_usd`, and `eco_mode_threshold_percent` (default 80%) (PR #22)
- **Eco mode** — Automatic degradation when approaching budget limits: forces cheap model tier, caps max_tokens to 1024, skips reflection, uses compact creed (PR #22)
- **Budget enforcement** — `BudgetEnforcer` with per-fighter and global limits, Warning/Blocked verdicts, daily cost tracking in f64 USD (PR #22)
- **Budget API** — `GET/PUT /api/budget`, `GET/PUT /api/budget/fighters/{id}` for runtime limit management (PR #22)

#### Desktop Automation

- **Multimodal pipeline** — Screenshot capture + vision analysis for desktop UI understanding (PR #14)
- **Desktop tools** — `sys_screenshot`, `ui_screenshot`, `app_ocr`, `ui_find_elements`, `ui_click`, `ui_type_text`, `ui_list_windows`, `ui_read_attribute` (PR #14)

#### Proactive Agents

- **Heartbeat scheduler** — Background task execution waking fighters on `every_bout`, `on_wake`, `hourly`, `daily` cadences (PR #15)
- **Channel notifications** — `channel_notify` tool pushes messages to Telegram/Slack/Discord without user prompting (PR #15)
- **Smart model routing** — `ModelRouter::classify()` with keyword heuristics routes to cheap/mid/expensive tiers automatically (PR #15)

#### Agent Self-Configuration

- **Self-config tools** — `heartbeat_add`, `heartbeat_list`, `heartbeat_remove`, `creed_view`, `skill_list`, `skill_recommend` gated by `Capability::SelfConfig` (PR #15)
- **Skill pack system** — MCP server config bundles: `productivity`, `developer`, `research`, `files` packs (PR #15)

### Fixed

- UTF-8 panic in `build_scan_text` tool result truncation (commit 1a72d54)
- Telegram photo handling end-to-end (PR #16)
- Conversation continuity, allowlist validation, and webhook flood control (commit 59a1408)
- `blocking_write()` panic in BudgetEnforcer — replaced `tokio::sync::RwLock` with `std::sync::RwLock` (PR #22)
- Small monthly budget limits truncating to 0 cents — switched from u64 cents to f64 USD (PR #22)
- Eco mode being a no-op without model routing configured (PR #22)

## [1.0.0] — 2026-03-16

### The First Punch

The initial public release of Punch — the agent operating system.

### Added

#### Core System

- **The Ring** — Central kernel coordinating all fighters, gorillas, troops, and system state
- **The Arena** — Full HTTP/WebSocket API with 25+ endpoint groups
- **Event Bus** — Broadcast-based pub/sub with 500-event history ring buffer
- **Live Dashboard** — Real-time WebSocket dashboard with event feed, fighter roster, gorilla status

#### Fighters (Interactive Agents)

- Spawn, kill, and message fighters via CLI or API
- 30 pre-configured fighter templates (researcher, coder, architect, security, etc.)
- Fighter-to-fighter direct messaging with automatic relationship tracking
- Multi-turn conversations between fighters
- OpenAI-compatible `/v1/chat/completions` endpoint

#### Gorillas (Autonomous Background Agents)

- 7 bundled gorillas: Alpha, Ghost, Prophet, Scout Troop, Swarm, Brawler, Howler
- Cron-based and human-readable scheduling (`every 30m`, `0 */6 * * *`)
- Autonomous tick execution with max iteration limits
- Global LLM concurrency control (semaphore-based)
- Graceful shutdown with watch channel signaling

#### Creeds (Agent Consciousness)

- Database-backed persistent identity documents
- Auto-creation on spawn with full self-awareness (model, provider, weight class)
- Learned behaviors with confidence decay and reinforcement
- Heartbeat tasks with cadences: `every_bout`, `on_wake`, `hourly`, `daily`
- Relationship tracking across fighter interactions
- Delegation rules for multi-agent collaboration
- Survives fighter kill/respawn cycles

#### Troops (Multi-Agent Coordination)

- 6 coordination strategies: LeaderWorker, RoundRobin, Broadcast, Pipeline, Consensus, Specialist
- Form, recruit, dismiss, assign tasks, disband
- Configurable task timeout (default 60s)
- Real result collection from all participants

#### Skills Marketplace

- 103 bundled skills across 12+ categories
- Git-indexed marketplace with Ed25519 signing and SHA-256 checksums
- 20+ rule security scanner (blocks pipe-to-shell, prompt injection, credential harvesting, Unicode obfuscation)
- Skill precedence: Workspace > Marketplace > User > Bundled
- CLI: `punch move search`, `install`, `scan`, `publish`, `verify`, `keygen`, `sync`, `lock`

#### Tools

- 70+ built-in tools: filesystem, shell, web, git, docker, browser, memory, crypto, templates
- Capability-based access control with pattern matching
- Secret bleed detection on shell commands
- SSRF protection blocking private IP ranges

#### LLM Providers

- 6 streaming drivers: Anthropic, OpenAI-compatible, Gemini, Ollama, AWS Bedrock, Azure OpenAI
- 15 providers supported through compatible endpoints
- Real streaming with SSE, NDJSON, and provider-specific protocols

#### Memory System

- SQLite-backed persistence with conversation history
- Knowledge graph (entities + relations)
- Memory decay with configurable exponential rates
- Context compaction (summarize or truncate strategies)
- AES-256-GCM encryption at rest

#### Channel Integrations

- 26 platform adapters: Telegram, Discord, Slack, WhatsApp, Signal, Matrix, Email, Teams, IRC, Reddit, Twitch, GitHub, LINE, Mastodon, Bluesky, LinkedIn, SMS, Google Chat, DingTalk, Feishu, Nostr, Mattermost, Zulip, Rocket.Chat, WebChat, Custom

#### Security (18 Layers)

- Capability-based access control with per-agent sandboxing
- Ed25519 request signing and AES-256-GCM encryption at rest
- Argon2id key derivation with zeroize-on-drop for all secrets
- Rate limiting, input sanitization, output filtering
- CORS validation, TLS enforcement, token rotation
- Gorilla containment zones and cross-troop privilege firewall
- Audit logging with correlation IDs

#### Federation & Interop

- P2P wire protocol with HMAC-SHA256 mutual authentication
- Google A2A (Agent-to-Agent) protocol support
- Agent card discovery at `/.well-known/agent.json`
- MCP (Model Context Protocol) server integration

#### Extensions

- WASM plugin system on Wasmtime 29
- Dual metering: fuel-based instruction limits + epoch-based wall-clock
- Whitelist permission model (network, filesystem, env vars, subprocess)
- Plugin registry with invocation tracking

#### Operations

- Multi-tenant support with per-tenant quotas and suspension
- Per-fighter budget enforcement with global and individual limits
- Prometheus-style metrics (tokens, duration, cost)
- Event-driven triggers with webhook endpoints
- Workflow engine for multi-step DAG execution
