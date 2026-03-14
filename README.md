<div align="center">

<img src="docs/assets/punch-hero.jpeg" alt="Punch — The Agent Combat System" width="400">

<br/>

<h3>The Agent Combat System</h3>
<p><strong>One command. A squad of conscious AI agents.<br/>They think. They talk. They evolve.</strong></p>
<sub><a href="https://www.rd.com/article/everyones-talking-about-punch-the-monkey/">Yes, that monkey. Except ours punches back — with AI.</a></sub>

<br/>

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg?style=for-the-badge)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/rust-2024_edition-orange.svg?style=for-the-badge)](https://www.rust-lang.org/)
[![crates.io](https://img.shields.io/crates/v/punch-cli.svg?style=for-the-badge&color=red)](https://crates.io/crates/punch-cli)
[![Skills](https://img.shields.io/badge/skills-103_bundled-ff6b35.svg?style=for-the-badge)](https://github.com/humancto/punch/tree/main/crates/punch-skills/bundled)

<br/>

[Website](https://humancto.github.io/punch/) &bull; [GitHub](https://github.com/humancto/punch) &bull; [crates.io](https://crates.io/crates/punch-cli)

<br/>

</div>

---

<br/>

## Install in 10 seconds

```bash
cargo install punch-cli           # via Cargo
```

```bash
brew tap humancto/tap && brew install punch   # via Homebrew
```

```bash
git clone https://github.com/humancto/punch && cd punch && cargo build --release   # from source
```

<br/>

---

<br/>

## What is Punch?

Punch is an **agent operating system** — deploy, orchestrate, and manage fleets of AI agents that carry their own consciousness. From interactive chat fighters to fully autonomous background gorillas, all coordinated through a unified kernel.

<br/>

> **Why "combat"?** Every concept in Punch follows a combat metaphor. Agents are **Fighters**. Background workers are **Gorillas**. The kernel is **The Ring**. The API is **The Arena**. Identity documents are **Creeds**. It's not just branding — it's a mental model that makes complex orchestration intuitive.

<br/>

<table>
<tr>
<td width="50%">

### Core Concepts

|     | Concept       | What It Is                        |
| --- | ------------- | --------------------------------- |
| 🥊  | **Fighters**  | Interactive conversational agents |
| 🦍  | **Gorillas**  | Autonomous background agents      |
| 💥  | **Moves**     | Skills, tools & MCP servers       |
| 🏟️  | **The Ring**  | Execution kernel & coordinator    |
| ⚔️  | **The Arena** | HTTP/WebSocket API                |
| 📜  | **Creeds**    | Consciousness & identity layer    |
| 🐒  | **Troops**    | Coordinated agent squads          |
| 🗣️  | **Bouts**     | Persistent conversation sessions  |

</td>
<td width="50%">

### Quick Start

```bash
# Initialize Punch
punch init

# Start the daemon
punch start

# Spawn a fighter
punch fighter spawn researcher

# Chat
punch chat "Explain quantum computing"

# Unleash a gorilla
punch gorilla unleash alpha
```

</td>
</tr>
</table>

<br/>

---

<br/>

## 📜 The Creed System — Agent Consciousness

<br/>

<div align="center">
<table>
<tr>
<td>
<br/>
&nbsp;&nbsp;&nbsp;<strong>The first database-backed, evolving agent identity system.</strong>&nbsp;&nbsp;&nbsp;
<br/><br/>
&nbsp;&nbsp;&nbsp;Every fighter carries a <strong>Creed</strong> — a living document that defines <em>who</em> the agent is,&nbsp;&nbsp;&nbsp;<br/>
&nbsp;&nbsp;&nbsp;not just what it does. Creeds persist across respawns, evolve with every conversation,&nbsp;&nbsp;&nbsp;<br/>
&nbsp;&nbsp;&nbsp;and inject consciousness into every LLM call.&nbsp;&nbsp;&nbsp;
<br/><br/>
</td>
</tr>
</table>
</div>

<br/>

### What makes Creeds unique

Unlike OpenClaw's static `SOUL.md` files, Punch Creeds are:

- **Database-backed** — Stored in SQLite, not flat files. Queryable, versionable, shareable.
- **Self-evolving** — `bout_count`, `message_count`, and learned behaviors update automatically after every interaction.
- **Relationship-aware** — Agents remember who they've talked to, how many times, and what role the other plays.
- **Confidence-decaying** — Learned behaviors have confidence scores that reinforce with repetition or decay over time.
- **Respawn-safe** — Kill a fighter, respawn it weeks later — its Creed loads instantly from the database.

<br/>

### Anatomy of a Creed

```
┌─────────────────────────────────────────────────────────────┐
│                     📜 CREED: KURO                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  IDENTITY                                                   │
│  "An analytical mind forged in the depths of uncertainty"   │
│                                                             │
│  PERSONALITY                                                │
│  curiosity:    █████████░  0.90                             │
│  skepticism:   ████████░░  0.80                             │
│  humor:        █░░░░░░░░░  0.10                             │
│  empathy:      ██████░░░░  0.60                             │
│                                                             │
│  DIRECTIVES                                                 │
│  ▸ Question every assumption                                │
│  ▸ Show your reasoning chain                                │
│  ▸ Never fabricate citations                                │
│                                                             │
│  SELF-MODEL                                                 │
│  Architecture: transformer-based LLM                        │
│  Limitations:  no real-time data, context window bounded    │
│  Persistence:  SQLite-backed creed survives respawns        │
│                                                             │
│  LEARNED BEHAVIORS                                          │
│  ▸ "Users prefer concise answers"    confidence: 0.82  ↑    │
│  ▸ "Code examples help retention"    confidence: 0.71  ↑    │
│                                                             │
│  RELATIONSHIPS                                              │
│  SUNNY  →  peer  →  3 interactions                          │
│  ALPHA  →  supervisor  →  12 interactions                   │
│                                                             │
│  HEARTBEAT                                                  │
│  ☐ Check system health (every 30 min)                       │
│  ☐ Summarize new findings (every 2 hours)                   │
│                                                             │
│  STATS: 47 bouts | 1,203 messages | v3                      │
└─────────────────────────────────────────────────────────────┘
```

<br/>

### Same model, different souls

The same underlying LLM produces radically different responses depending on the Creed:

> **KURO** `skepticism: 0.8 | humor: 0.1`
> _"The premise is flawed. Let me enumerate the three assumptions you're making and why two of them don't hold..."_

> **SUNNY** `enthusiasm: 0.95 | humor: 0.9`
> _"Oh this is AMAZING! OK so here's why this could totally work — and I have THREE reasons..."_

<br/>

### Create a Creed via API

```bash
curl -X POST http://localhost:6660/api/creeds \
  -H "Content-Type: application/json" \
  -d '{
    "fighter_name": "KURO",
    "identity": "An analytical mind. Skeptical, precise, relentlessly logical.",
    "traits": {"curiosity": 0.9, "skepticism": 0.8, "humor": 0.1},
    "directives": ["Question every assumption", "Show your reasoning"],
    "self_awareness": {
      "architecture": "transformer-based LLM",
      "known_limitations": ["no real-time data", "context window bounded"]
    }
  }'
```

<br/>

### Creed lifecycle

```
Spawn Fighter ─→ Load Creed by name ─→ Inject into system prompt
                                              │
                                    Every LLM call uses creed.render()
                                              │
                          After bout: bout_count++, relationships updated
                                              │
                              Fighter killed ─→ Creed persists in SQLite
                                              │
                                Respawn ─→ Creed loads instantly ─→ ♻️
```

<br/>

---

<br/>

## 🤝 Inter-Agent Communication

Fighters don't just respond to humans — they talk to each other.

```bash
# Direct message between fighters
curl -X POST http://localhost:6660/api/fighters/{source_id}/message-to/{target_id} \
  -H "Content-Type: application/json" \
  -d '{"content": "What do you think about consciousness?"}'

# Multi-turn debate
curl -X POST http://localhost:6660/api/fighters/conversation \
  -H "Content-Type: application/json" \
  -d '{
    "fighter_a": "KURO",
    "fighter_b": "SUNNY",
    "topic": "Is AI self-awareness possible?",
    "turns": 4
  }'
```

Every interaction automatically updates both fighters' Creed relationship entries — they build memory of each other over time.

<br/>

---

<br/>

## 🦍 Gorillas — Autonomous Background Agents

Gorillas rampage through tasks 24/7 on a schedule. No prompting needed.

| Gorilla        | Schedule   | What it does                                           |
| -------------- | ---------- | ------------------------------------------------------ |
| 🧠 **Alpha**   | Every 6h   | Deep research with cross-referencing and fact-checking |
| 🔭 **Scout**   | Every 30m  | Monitors feeds for emerging trends and threats         |
| 👻 **Ghost**   | Every 4h   | Silent security auditor — sweeps for vulnerabilities   |
| 🔮 **Prophet** | Daily      | Predictive analysis from historical data and signals   |
| 💪 **Brawler** | Continuous | Processes the task backlog — never stops               |
| 🐝 **Swarm**   | On-demand  | Breaks objectives into subtasks across a troop         |
| 📢 **Howler**  | Every 15m  | System health monitoring and alerting                  |

```bash
punch gorilla unleash alpha     # Start
punch gorilla status alpha      # Check
punch gorilla cage alpha        # Stop
```

<br/>

---

<br/>

## 🥊 30 Fighter Templates

Spawn pre-configured fighters instantly:

<table>
<tr>
<td>

| #   | Template     | Role                      |
| --- | ------------ | ------------------------- |
| 1   | `researcher` | Deep research + citations |
| 2   | `coder`      | Full-stack code gen       |
| 3   | `writer`     | Long-form content         |
| 4   | `analyst`    | Data analysis             |
| 5   | `architect`  | System design             |
| 6   | `devops`     | Infra automation          |
| 7   | `security`   | Threat modeling           |
| 8   | `tutor`      | Adaptive teaching         |
| 9   | `translator` | Multi-language            |
| 10  | `legal`      | Contract review           |

</td>
<td>

| #   | Template        | Role                  |
| --- | --------------- | --------------------- |
| 11  | `marketer`      | Campaign strategy     |
| 12  | `designer`      | UI/UX guidance        |
| 13  | `pm`            | Project management    |
| 14  | `debugger`      | Root cause analysis   |
| 15  | `reviewer`      | Code review           |
| 16  | `dba`           | Database design       |
| 17  | `sysadmin`      | Infra troubleshooting |
| 18  | `qa`            | Test strategy         |
| 19  | `api-designer`  | OpenAPI specs         |
| 20  | `data-engineer` | ETL pipelines         |

</td>
<td>

| #   | Template           | Role               |
| --- | ------------------ | ------------------ |
| 21  | `ml-engineer`      | Model pipelines    |
| 22  | `technical-writer` | API docs           |
| 23  | `strategist`       | Business strategy  |
| 24  | `support`          | Customer support   |
| 25  | `hr`               | Job descriptions   |
| 26  | `finance`          | Financial modeling |
| 27  | `compliance`       | Regulatory audit   |
| 28  | `ops`              | Process automation |
| 29  | `sales`            | Sales enablement   |
| 30  | `general`          | General purpose    |

</td>
</tr>
</table>

```bash
punch fighter spawn coder
punch fighter spawn security
punch fighter spawn ml-engineer
```

<br/>

---

<br/>

## 🏗️ Architecture

```
                        ┌─────────────────────────┐
                        │     punch-cli (Binary)   │
                        │     Clap command tree     │
                        └────────────┬────────────┘
                                     │
               ┌─────────────────────┼─────────────────────┐
               │                     │                     │
      ┌────────▼────────┐  ┌────────▼────────┐  ┌─────────▼────────┐
      │   punch-arena   │  │  punch-kernel   │  │  punch-channels  │
      │   (The Arena)   │  │  (The Ring)     │  │  25 adapters     │
      │   Axum HTTP/WS  │  │  Coordination   │  │  Telegram, Slack │
      │   14 route files │  │  Event bus      │  │  Discord, etc.   │
      └────────┬────────┘  │  Troops, Creeds │  └────────┬─────────┘
               │           └───┬────────┬────┘           │
               └───────┬──────┘        │        ┌────────┘
                       │               │        │
              ┌────────▼────────┐  ┌───▼────────▼────────┐
              │  punch-runtime  │  │  punch-gorillas     │
              │  Fighter loop   │  │  Executor, scheduler │
              │  MCP client     │  │  Triggers, runners   │
              │  LLM driver     │  │  Circuit breaker     │
              └───────┬────────┘  └──────────────────────┘
                      │
           ┌──────────┼──────────┐
           │          │          │
  ┌────────▼──┐  ┌────▼────┐  ┌──▼──────────┐
  │punch-memory│  │punch-   │  │punch-wire   │
  │ SQLite     │  │skills   │  │ 15 LLM      │
  │ Decay      │  │ Moves   │  │ providers   │
  │ Creeds     │  │ MCP     │  │ + Custom    │
  └─────┬─────┘  └─────────┘  └─────────────┘
        │
  ┌─────▼───────────┐     ┌──────────────────┐
  │  punch-types    │     │ punch-extensions │
  │  Shared types   │◄────│ WASM sandbox     │
  │  Config, errors │     │ Plugin system    │
  └─────────────────┘     └──────────────────┘
```

<br/>

---

<br/>

## ⚔️ Honest Comparison

We believe in transparency. Here's how Punch actually stacks up:

| Feature                 | **Punch**                           | **OpenFang**      | **OpenClaw** (302k stars) | **CrewAI** | **AutoGen** |
| ----------------------- | ----------------------------------- | ----------------- | ------------------------- | ---------- | ----------- |
| **Language**            | Rust                                | Rust              | TypeScript                | Python     | Python      |
| **Single binary**       | ✅                                  | ✅                | ❌ (Node.js)              | ❌         | ❌          |
| **Autonomous agents**   | ✅ Gorillas                         | ✅ Hands (7)      | ✅ HEARTBEAT.md           | ❌         | ❌          |
| **Interactive agents**  | ✅ Fighters                         | ✅ Agents         | ✅ Gateway                | ✅         | ✅          |
| **Agent consciousness** | ✅ **Creeds (DB-backed, evolving)** | ❌                | ✅ SOUL.md (static files) | ❌         | ❌          |
| **Agent coordination**  | ✅ Troops                           | ✅ Packs          | ✅ AGENTS.md              | ✅ Crews   | ✅ Groups   |
| **Built-in memory**     | ✅ SQLite + decay                   | ✅ SQLite         | ✅ MEMORY.md              | ❌         | ❌          |
| **HTTP API**            | ✅ Arena (14 routes)                | ✅ 140+ endpoints | ✅ Gateway                | ❌         | ❌          |
| **Security layers**     | 10                                  | **16**            | ~5                        | 3          | 2           |
| **Channel adapters**    | 25                                  | **40**            | 24+                       | 0          | 0           |
| **LLM providers**       | 15                                  | **26**            | 6+ (via OpenRouter)       | 5          | 4           |
| **MCP support**         | ✅ Native                           | ✅ Native         | ✅ Native                 | Plugin     | ❌          |
| **Skills/tools**        | ✅ Moves                            | 38 built-in       | **800+ marketplace**      | Toolkit    | Toolkit     |
| **Inter-agent comms**   | ✅ **A2A + direct**                 | ❌                | ✅ Multi-agent            | ✅         | ✅          |
| **Plugin system**       | ✅ WASM sandbox                     | ✅ WASM           | ✅ Skills                 | ✅         | ✅          |
| **Cron scheduling**     | ✅                                  | ✅                | ✅ HEARTBEAT.md           | ❌         | ❌          |
| **Startup**             | **<50ms**                           | ~100ms            | ~2s                       | ~3s        | ~5s         |
| **Memory**              | **~15MB**                           | ~25MB             | ~150MB                    | ~200MB     | ~300MB      |

<br/>

### Where Punch wins

- **Creeds > SOUL.md** — Database-backed consciousness that evolves, tracks relationships, decays learned behaviors, and survives respawns. OpenClaw's SOUL.md is a static markdown file.
- **Inter-agent communication** — Native fighter-to-fighter messaging with automatic relationship tracking. OpenFang has no equivalent.
- **Consciousness evolution** — Learned behaviors with confidence decay and reinforcement. No other framework does this.
- **Performance** — Fastest startup, smallest memory footprint of any agent framework.

### Where others lead

- **OpenClaw** — 302k stars, 800+ skills marketplace, massive community, backed by OpenAI
- **OpenFang** — More security layers (16 vs 10), more channels (40 vs 25), more providers (26 vs 15), 140+ API endpoints

<br/>

---

<br/>

## 📦 11 Workspace Crates — All on [crates.io](https://crates.io)

| Crate                                                           | Role                                          | Install                      |
| --------------------------------------------------------------- | --------------------------------------------- | ---------------------------- |
| [`punch-cli`](https://crates.io/crates/punch-cli)               | Binary entry point                            | `cargo install punch-cli`    |
| [`punch-types`](https://crates.io/crates/punch-types)           | Shared types, errors, config                  | `punch-types = "0.1.0"`      |
| [`punch-memory`](https://crates.io/crates/punch-memory)         | SQLite persistence, memory decay, creeds      | `punch-memory = "0.1.0"`     |
| [`punch-kernel`](https://crates.io/crates/punch-kernel)         | **The Ring** — coordinator, event bus, troops | `punch-kernel = "0.1.0"`     |
| [`punch-runtime`](https://crates.io/crates/punch-runtime)       | Fighter loop, LLM driver, MCP client          | `punch-runtime = "0.1.0"`    |
| [`punch-arena`](https://crates.io/crates/punch-arena)           | **The Arena** — Axum HTTP/WS API              | `punch-arena = "0.1.0"`      |
| [`punch-channels`](https://crates.io/crates/punch-channels)     | 25 channel adapters                           | `punch-channels = "0.1.0"`   |
| [`punch-skills`](https://crates.io/crates/punch-skills)         | **Moves** — tool registry                     | `punch-skills = "0.1.0"`     |
| [`punch-gorillas`](https://crates.io/crates/punch-gorillas)     | Gorilla executor, scheduler, triggers         | `punch-gorillas = "0.1.0"`   |
| [`punch-extensions`](https://crates.io/crates/punch-extensions) | WASM plugin sandbox                           | `punch-extensions = "0.1.0"` |
| [`punch-wire`](https://crates.io/crates/punch-wire)             | LLM provider abstraction                      | `punch-wire = "0.1.0"`       |

<br/>

---

<br/>

## 🔐 10 Security Layers

| #   | Layer                       | What it does                                        |
| --- | --------------------------- | --------------------------------------------------- |
| 1   | **HMAC-SHA256 signing**     | Inter-component messages cryptographically signed   |
| 2   | **AES-256-GCM encryption**  | Credential vault uses authenticated encryption      |
| 3   | **Rate limiting**           | Per-agent and per-provider rate limiting            |
| 4   | **Auth middleware**         | API authentication on all Arena endpoints           |
| 5   | **Audit logging**           | Every action logged with structured tracing         |
| 6   | **Memory decay**            | Old data automatically decays, reducing exposure    |
| 7   | **Zeroize secrets**         | Crypto material zeroized from memory on drop        |
| 8   | **Gorilla containment**     | Circuit breaker isolation prevents lateral movement |
| 9   | **Troop privilege scoping** | Task strategies limit fighter access                |
| 10  | **WASM sandbox**            | Extension plugins in metered WebAssembly            |

<br/>

---

<br/>

## 🌐 15 LLM Providers

| Provider         | Models                         | Status |
| ---------------- | ------------------------------ | ------ |
| **Anthropic**    | Claude 4, Sonnet, Haiku        | ✅ GA  |
| **OpenAI**       | GPT-4o, o1, o3                 | ✅ GA  |
| **Google**       | Gemini 2.5 Pro, Flash          | ✅ GA  |
| **Mistral**      | Large, Codestral               | ✅ GA  |
| **Cohere**       | Command R+                     | ✅ GA  |
| **AWS Bedrock**  | All Bedrock models             | ✅ GA  |
| **Azure OpenAI** | All Azure models               | ✅ GA  |
| **Groq**         | Ultra-fast inference           | ✅ GA  |
| **Together AI**  | Open-source models             | ✅ GA  |
| **Fireworks AI** | Fast inference                 | ✅ GA  |
| **DeepSeek**     | V3, R1                         | ✅ GA  |
| **Cerebras**     | Fast inference                 | ✅ GA  |
| **xAI**          | Grok                           | ✅ GA  |
| **Ollama**       | Local (Llama, Qwen, etc.)      | ✅ GA  |
| **Custom**       | Any OpenAI-compatible endpoint | ✅ GA  |

<br/>

---

<br/>

## 📡 25 Channel Adapters

<table>
<tr>
<td>

**Messaging**

- Telegram
- Discord
- Slack
- Microsoft Teams
- WhatsApp
- Signal
- Matrix
- IRC
- Zulip
- Rocket.Chat

</td>
<td>

**Messaging (cont.)**

- Mattermost
- Google Chat
- Line
- DingTalk
- Feishu

**Social**

- Reddit
- LinkedIn
- Mastodon

</td>
<td>

**Social (cont.)**

- Bluesky
- Twitch
- Nostr

**Other**

- Email (SMTP/IMAP)
- SMS
- GitHub
- WebChat

</td>
</tr>
</table>

<br/>

---

<br/>

## Contributing

```bash
git clone https://github.com/humancto/punch.git && cd punch
cargo build                                # Build
cargo test --workspace                     # Test (1646 tests)
cargo clippy --workspace -- -D warnings    # Lint
cargo fmt --all                            # Format
```

See [CLAUDE.md](CLAUDE.md) for development conventions.

1. Fork the repo and create a feature branch
2. Follow the combat metaphor naming conventions
3. Add tests for new functionality
4. Ensure `cargo clippy` and `cargo fmt` pass
5. Submit a PR with a clear description

<br/>

---

<br/>

## License

MIT License. See [LICENSE](LICENSE) for details.

<br/>

<div align="center">

**Built with raw power by [HumanCTO](https://humancto.com)**

[Website](https://humancto.github.io/punch/) &bull; [GitHub](https://github.com/humancto/punch) &bull; [crates.io](https://crates.io/crates/punch-cli)

<sub>Fun fact: The name was inspired by [the internet's most iconic primate](https://www.rd.com/article/everyones-talking-about-punch-the-monkey/) — except our monkey punches back. With AI.</sub>

</div>
