```
                                      ____  __  ___   ________  __
                                     / __ \/ / / / | / / ____/ / /
                                    / /_/ / / / /  |/ / /     / /_
                                   / ____/ /_/ / /|  / /___  / __ \
                                  /_/    \____/_/ |_/\____/ /_/ /_/

                          ╔══════════════════════════════════════════════╗
                          ║                                              ║
                          ║    🥊  THE AGENT COMBAT SYSTEM  🦍          ║
                          ║                                              ║
                          ║    Deploy autonomous AI agent squads          ║
                          ║    from a single binary.                      ║
                          ║                                              ║
                          ╚══════════════════════════════════════════════╝

                                        ╭━━━╮
                                       ╭╯ ● ● ╰╮
                                       │  ━━━  │
                                       ╰┬─────┬╯
                                     ╭──┤     ├──╮
                                    ╱│  │     │  │╲
                                   🥊│  │     │  │🥊
                                     ╰──┤     ├──╯
                                        │     │
                                       ╱│     │╲
                                      ╱ ╰─────╯ ╲
                                     ╱           ╲
```

<p align="center">
  <strong>The Agent Combat System — Deploy autonomous AI agent squads from a single binary.</strong>
</p>

<p align="center">
  <a href="https://github.com/humancto/punch/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-2024_edition-orange.svg" alt="Rust"></a>
  <a href="https://github.com/humancto/punch/actions"><img src="https://img.shields.io/badge/tests-passing-brightgreen.svg" alt="Tests"></a>
  <a href="https://punch.sh"><img src="https://img.shields.io/badge/docs-punch.sh-blueviolet.svg" alt="Docs"></a>
  <a href="https://github.com/humancto/punch/releases"><img src="https://img.shields.io/badge/version-0.1.0-red.svg" alt="Version"></a>
</p>

---

## One-Liner Install

```bash
curl -fsSL https://punch.sh/install | sh
```

Or build from source:

```bash
git clone https://github.com/humancto/punch.git
cd punch
cargo build --release
```

---

## What is Punch?

Punch is a **single-binary agent operating system** built in Rust. It lets you deploy, orchestrate, and manage fleets of AI agents — from interactive chat assistants to fully autonomous background workers — all coordinated through a unified kernel architecture.

Everything in Punch follows a **combat metaphor**:

| Concept          | What It Is            | Description                                                                                                  |
| ---------------- | --------------------- | ------------------------------------------------------------------------------------------------------------ |
| 🥊 **Fighters**  | Conversational agents | AI agents you spar with — chat, command, delegate tasks. Interactive and responsive.                         |
| 🦍 **Gorillas**  | Autonomous agents     | Background agents that rampage through tasks 24/7 on a schedule, no prompting needed.                        |
| 💥 **Moves**     | Skills & tools        | Capabilities that fighters and gorillas wield — web search, file I/O, code execution, MCP servers, and more. |
| 🏟️ **The Ring**  | Execution kernel      | The central coordinator where all agent execution happens. Manages lifecycle, quotas, and invariants.        |
| ⚔️ **The Arena** | HTTP API              | RESTful + WebSocket API for external integration. Connect anything to your agent squads.                     |
| 🗣️ **Bouts**     | Conversation sessions | Persistent conversation sessions with full memory, context windowing, and recall.                            |
| 🔗 **Combos**    | Chained workflows     | Multi-step agent pipelines — output of one agent feeds the next.                                             |
| 🐒 **Troops**    | Coordinated squads    | Groups of agents working together on complex objectives with shared context.                                 |

---

## Quick Start

```bash
# 1. Initialize Punch
punch init

# 2. Start the daemon (Ring + Arena)
punch start

# 3. Spawn a fighter from a template
punch fighter spawn researcher

# 4. Chat with your fighter
punch chat "What are the latest developments in quantum computing?"

# Or interactive mode
punch fighter chat
```

---

## 🦍 Gorilla Showcase

Gorillas are autonomous agents that operate on schedules, executing tasks without human prompting. Unleash them and let them rampage.

| Gorilla            | Schedule          | Description                                                                                                                                                                          |
| ------------------ | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 🧠 **The Alpha**   | Every 6 hours     | Deep researcher that cross-references sources, fact-checks claims, and produces comprehensive research reports with citations. Skeptical by nature — every claim gets verified.      |
| 🔭 **Scout Troop** | Every 30 min      | Reconnaissance gorilla that monitors RSS feeds, social media, and news sources for emerging trends, threats, and opportunities. First to know, first to report.                      |
| 👻 **Ghost**       | Every 4 hours     | Silent auditor that sweeps your codebase, infrastructure, and configurations for security vulnerabilities, misconfigurations, and compliance violations. You'll never see it coming. |
| 🔮 **Prophet**     | Daily at midnight | Forecasting gorilla that analyzes historical data, market trends, and signals to generate predictive reports. Sees what others miss.                                                 |
| 💪 **Brawler**     | Continuous        | The workhorse. Processes queued tasks from the backlog — data transforms, batch operations, file processing. Never stops, never complains.                                           |
| 🐝 **Swarm**       | On-demand         | Coordinator that breaks complex objectives into subtasks and distributes them across a troop of fighters. Divide and conquer at scale.                                               |
| 📢 **Howler**      | Every 15 min      | Notification gorilla that monitors system health, agent metrics, and configured alerts. When something needs attention, Howler makes sure you know.                                  |

```bash
# Unleash a gorilla
punch gorilla unleash alpha

# Check status
punch gorilla status alpha

# Cage it when done
punch gorilla cage alpha
```

---

## 🥊 30 Fighter Templates

Spawn any of these pre-configured fighters instantly:

| #   | Template           | Description                                                 |
| --- | ------------------ | ----------------------------------------------------------- |
| 1   | `researcher`       | Deep research with source verification and citations        |
| 2   | `coder`            | Full-stack code generation, review, and debugging           |
| 3   | `writer`           | Long-form content creation with style adaptation            |
| 4   | `analyst`          | Data analysis, visualization, and insight extraction        |
| 5   | `architect`        | System design and technical architecture planning           |
| 6   | `devops`           | Infrastructure, CI/CD, and deployment automation            |
| 7   | `security`         | Security analysis, pen-test planning, and threat modeling   |
| 8   | `tutor`            | Personalized teaching with adaptive difficulty              |
| 9   | `translator`       | Multi-language translation with cultural context            |
| 10  | `legal`            | Contract review, compliance checking, and legal research    |
| 11  | `marketer`         | Campaign strategy, copywriting, and audience analysis       |
| 12  | `designer`         | UI/UX design guidance and design system management          |
| 13  | `pm`               | Project management, sprint planning, and stakeholder comms  |
| 14  | `debugger`         | Systematic bug diagnosis and root cause analysis            |
| 15  | `reviewer`         | Code review with style, security, and performance checks    |
| 16  | `dba`              | Database design, query optimization, and migration planning |
| 17  | `sysadmin`         | System administration and infrastructure troubleshooting    |
| 18  | `qa`               | Test strategy, test case generation, and coverage analysis  |
| 19  | `api-designer`     | API design, OpenAPI spec generation, and documentation      |
| 20  | `data-engineer`    | ETL pipeline design, data modeling, and orchestration       |
| 21  | `ml-engineer`      | Model training, evaluation, and deployment pipelines        |
| 22  | `technical-writer` | API docs, user guides, and knowledge base articles          |
| 23  | `strategist`       | Business strategy, competitive analysis, and roadmapping    |
| 24  | `support`          | Customer support with knowledge base integration            |
| 25  | `hr`               | Job descriptions, interview questions, and policy drafting  |
| 26  | `finance`          | Financial modeling, budgeting, and forecasting              |
| 27  | `compliance`       | Regulatory compliance checking and audit preparation        |
| 28  | `ops`              | Operations optimization and process automation              |
| 29  | `sales`            | Sales enablement, prospecting research, and outreach drafts |
| 30  | `general`          | General-purpose assistant with balanced capabilities        |

```bash
punch fighter spawn coder
punch fighter spawn security
punch fighter spawn ml-engineer
```

---

## Architecture

```
                              ┌─────────────────────┐
                              │    punch-cli (CLI)   │
                              │   Clap command tree  │
                              └──────────┬──────────┘
                                         │
                    ┌────────────────────┼────────────────────┐
                    │                    │                    │
           ┌───────▼───────┐  ┌─────────▼────────┐  ┌───────▼────────┐
           │  punch-api    │  │  punch-kernel     │  │ punch-channels │
           │  (The Arena)  │  │  (The Ring)       │  │ (Adapters)     │
           │  Axum HTTP/WS │  │  Central coord.   │  │ Telegram,      │
           └───────┬───────┘  │  Event bus,       │  │ Discord, etc.  │
                   │          │  Scheduler         │  └───────┬────────┘
                   │          └────┬──────┬────────┘          │
                   │               │      │                   │
                   └───────┬───────┘      │          ┌────────┘
                           │              │          │
                  ┌────────▼────────┐  ┌──▼──────────▼───┐
                  │  punch-runtime  │  │  punch-gorillas  │
                  │  Fighter loop,  │  │  Gorilla loader, │
                  │  LLM driver     │  │  manifests,      │
                  │                 │  │  scheduler        │
                  └───────┬────────┘  └──────────────────┘
                          │
              ┌───────────┼───────────┐
              │           │           │
     ┌────────▼──┐  ┌─────▼─────┐  ┌──▼──────────┐
     │punch-memory│  │punch-skills│  │punch-wire   │
     │ SQLite,    │  │ (Moves)   │  │ LLM provider │
     │ decay,     │  │ Tool reg, │  │ abstraction  │
     │ compaction │  │ MCP client│  │ 27+ providers│
     └────────┬──┘  └───────────┘  └─────────────┘
              │
     ┌────────▼──────────┐
     │   punch-types     │
     │   Shared types,   │
     │   errors, config  │
     └───────────────────┘
              │
     ┌────────▼──────────┐
     │  punch-extensions │
     │  Plugin system    │
     └───────────────────┘
```

---

## Comparison

| Feature                | **Punch**         | OpenFang   | CrewAI   | AutoGen   | LangGraph |
| ---------------------- | ----------------- | ---------- | -------- | --------- | --------- |
| **Language**           | Rust              | Rust       | Python   | Python    | Python    |
| **Single binary**      | ✅                | ✅         | ❌       | ❌        | ❌        |
| **Autonomous agents**  | ✅ Gorillas       | ✅ Daemons | ❌       | ❌        | ❌        |
| **Interactive agents** | ✅ Fighters       | ✅ Agents  | ✅       | ✅        | ✅        |
| **Agent coordination** | ✅ Troops         | ✅ Packs   | ✅ Crews | ✅ Groups | ✅ Graphs |
| **Built-in memory**    | ✅ SQLite + decay | ✅         | ❌       | ❌        | ❌        |
| **HTTP API**           | ✅ Arena          | ✅         | ❌       | ❌        | ✅        |
| **Security layers**    | **18**            | 16         | 3        | 2         | 4         |
| **Channel adapters**   | **50+ planned**   | 12         | 0        | 0         | 0         |
| **LLM providers**      | **27+**           | 15         | 5        | 4         | 3         |
| **MCP support**        | ✅ Native         | ✅         | Plugin   | ❌        | ❌        |
| **Startup time**       | <50ms             | ~100ms     | ~3s      | ~5s       | ~4s       |
| **Memory usage**       | ~15MB             | ~25MB      | ~200MB   | ~300MB    | ~250MB    |
| **Plugin system**      | ✅ Extensions     | ✅         | ✅       | ✅        | ✅        |
| **Cron scheduling**    | ✅                | ✅         | ❌       | ❌        | ❌        |

---

## 📦 Workspace Crates

Punch is a Cargo workspace with **12 crates**, each with a single responsibility:

| Crate              | Role                                                     | Key Dependencies                      |
| ------------------ | -------------------------------------------------------- | ------------------------------------- |
| `punch-cli`        | Binary entry point, Clap command tree                    | `clap`, `punch-kernel`, `punch-api`   |
| `punch-types`      | Shared types, errors, config structs                     | `serde`, `thiserror`, `uuid`          |
| `punch-memory`     | SQLite persistence, memory decay, compaction             | `rusqlite`, `chrono`, `punch-types`   |
| `punch-kernel`     | **The Ring** — central coordinator, event bus, scheduler | `dashmap`, `tokio`, `punch-runtime`   |
| `punch-runtime`    | Fighter loop execution, LLM driver trait                 | `tokio`, `punch-wire`, `punch-memory` |
| `punch-api`        | **The Arena** — Axum HTTP/WS API                         | `axum`, `tower-http`, `punch-kernel`  |
| `punch-channels`   | Channel adapters (Telegram, Discord, Slack, ...)         | `reqwest`, `punch-kernel`             |
| `punch-skills`     | **Moves** — tool registry, MCP client                    | `serde_json`, `punch-types`           |
| `punch-gorillas`   | Gorilla loader, bundled gorilla manifests                | `toml`, `punch-types`                 |
| `punch-extensions` | Plugin system for third-party extensions                 | `punch-types`                         |
| `punch-wire`       | LLM provider abstraction (27+ providers)                 | `reqwest`, `serde_json`               |
| `xtask`            | Build automation and dev tooling                         | —                                     |

---

## 🔐 Security

Punch ships with **18 security layers** — more than any competing agent framework:

1. **Capability-based access control** — Agents only get the permissions they're explicitly granted
2. **Per-agent sandboxing** — Each fighter runs in an isolated capability space
3. **API key vault** — Secrets loaded from environment variables, never stored in config
4. **Ed25519 request signing** — All inter-component messages are cryptographically signed
5. **AES-256-GCM encryption at rest** — Memory substrate encrypted with authenticated encryption
6. **Argon2id key derivation** — Master keys derived with memory-hard KDF
7. **Rate limiting & quotas** — Per-agent and per-provider rate limiting via the Scheduler
8. **Input sanitization** — All user inputs sanitized before reaching the LLM
9. **Output filtering** — Agent outputs filtered for sensitive data leakage
10. **Audit logging** — Every agent action logged with full traceability
11. **TLS-only external comms** — All outbound connections use rustls
12. **CORS & origin validation** — Arena API validates request origins
13. **Token rotation** — Automatic rotation of session tokens
14. **Memory decay** — Old conversation data automatically decays, reducing exposure window
15. **Zeroize secrets** — Cryptographic material zeroized from memory when dropped
16. **Resource limits** — CPU, memory, and network quotas per agent
17. **Gorilla containment zones** — Each gorilla runs in an isolated execution environment with its own capability boundary, preventing lateral movement between autonomous agents
18. **Cross-troop privilege firewall** — Troops cannot escalate each other's capabilities; a troop's combined permissions are the intersection (not union) of its members' grants

See [docs/security.md](docs/security.md) for the full security architecture.

---

## 🌐 LLM Provider Support

Punch supports **27+ LLM providers** out of the box through `punch-wire`:

| Provider     | Models                                | Status |
| ------------ | ------------------------------------- | ------ |
| Anthropic    | Claude 4, Claude Sonnet, Claude Haiku | ✅ GA  |
| OpenAI       | GPT-4o, GPT-4o-mini, o1, o3           | ✅ GA  |
| Google       | Gemini 2.5 Pro, Flash                 | ✅ GA  |
| Meta         | Llama 3.3, 4                          | ✅ GA  |
| Mistral      | Mistral Large, Codestral              | ✅ GA  |
| Cohere       | Command R+                            | ✅ GA  |
| AWS Bedrock  | All Bedrock models                    | ✅ GA  |
| Azure OpenAI | All Azure-hosted models               | ✅ GA  |
| Groq         | Ultra-fast inference                  | ✅ GA  |
| Together AI  | Open-source models                    | ✅ GA  |
| Fireworks AI | Fast open-source inference            | ✅ GA  |
| Perplexity   | Search-augmented models               | ✅ GA  |
| DeepSeek     | DeepSeek V3, R1                       | ✅ GA  |
| Ollama       | Local models                          | ✅ GA  |
| LM Studio    | Local models                          | ✅ GA  |
| vLLM         | Self-hosted inference                 | ✅ GA  |
| OpenRouter   | Multi-provider routing                | ✅ GA  |
| Replicate    | Model marketplace                     | ✅ GA  |
| Anyscale     | Scalable inference                    | ✅ GA  |
| Databricks   | DBRX, hosted models                   | ✅ GA  |
| AI21         | Jamba                                 | ✅ GA  |
| Reka         | Reka Core                             | ✅ GA  |
| Moonshot     | Kimi                                  | ✅ GA  |
| Zhipu AI     | GLM-4                                 | ✅ GA  |
| Baichuan     | Baichuan models                       | ✅ GA  |
| xAI          | Grok                                  | ✅ GA  |
| NVIDIA NIM   | Self-hosted NVIDIA models             | ✅ GA  |

Any OpenAI-compatible endpoint works automatically.

---

## 📡 Channel Adapters

**50+ channel adapters** planned for `punch-channels`:

**Messaging:** Telegram, Discord, Slack, Microsoft Teams, WhatsApp, Signal, Matrix, IRC, XMPP, Zulip, Rocket.Chat, Mattermost

**Email:** SMTP, IMAP, Gmail API, Outlook API, SendGrid, Mailgun, Postmark, SES

**Voice:** Twilio, Vonage, Daily.co, LiveKit

**Web:** REST webhooks, WebSocket, Server-Sent Events, GraphQL subscriptions

**Social:** Twitter/X, Reddit, LinkedIn, Facebook Messenger, Instagram DM

**Developer:** GitHub Issues/PRs, GitLab, Jira, Linear, Notion, Confluence

**Custom:** Any HTTP endpoint, gRPC, MQTT, AMQP, NATS, Redis Pub/Sub, Kafka

---

## Contributing

We welcome contributions! Here's how to get started:

```bash
# Clone and build
git clone https://github.com/humancto/punch.git
cd punch

# Build all crates
cargo build

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all
```

See [CLAUDE.md](CLAUDE.md) for development conventions and architecture details.

### Contribution Guidelines

1. Fork the repo and create a feature branch
2. Follow the combat metaphor naming conventions
3. Add tests for new functionality
4. Ensure `cargo clippy` and `cargo fmt` pass
5. Submit a PR with a clear description

---

## License

MIT License. See [LICENSE](LICENSE) for details.

---

<p align="center">
  <strong>Built with 🦀 Rust and 🦍 raw power by <a href="https://humancto.com">HumanCTO</a></strong>
</p>

<p align="center">
  <a href="https://punch.sh">Website</a> · <a href="https://github.com/humancto/punch">GitHub</a> · <a href="https://discord.gg/punch">Discord</a> · <a href="https://twitter.com/punchagents">Twitter</a>
</p>
