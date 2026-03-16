# Changelog

All notable changes to Punch will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-03-16

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
