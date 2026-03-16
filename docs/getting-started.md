# Getting Started with Punch

This guide walks you through installing Punch, spawning your first fighter, running an autonomous gorilla, and setting up multi-agent coordination — all in under 10 minutes.

## Prerequisites

- **Rust 2024 edition** (1.85+) if building from source
- **An LLM provider** — either a local [Ollama](https://ollama.com) instance or an API key for Anthropic/OpenAI/etc.

## Step 1: Install

Choose your method:

```bash
# Via Cargo (recommended)
cargo install punch-cli

# Via Homebrew
brew tap humancto/tap && brew install punch

# From source
git clone https://github.com/humancto/punch && cd punch && cargo build --release
```

Verify:

```bash
punch --version
```

## Step 2: Initialize

```bash
punch init
```

This creates `~/.punch/` with a default configuration file. Edit `~/.punch/config.toml` to set your LLM provider:

### Option A: Local Ollama (free, private)

```toml
api_listen = "127.0.0.1:6660"

[default_model]
provider = "ollama"
model = "qwen3:8b"
base_url = "http://localhost:11434"
max_tokens = 4096
temperature = 0.7
```

Make sure Ollama is running: `ollama serve`

### Option B: Anthropic Claude

```toml
api_listen = "127.0.0.1:6660"

[default_model]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
```

Then export your key: `export ANTHROPIC_API_KEY=sk-ant-...`

### Option C: OpenAI

```toml
api_listen = "127.0.0.1:6660"

[default_model]
provider = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
```

## Step 3: Start the Daemon

```bash
punch start
```

Punch is now running. Open the dashboard at **http://127.0.0.1:6660/dashboard** to see everything in real-time.

## Step 4: Spawn Your First Fighter

```bash
punch fighter spawn researcher
```

This creates a fighter using the `researcher` template — a deep research agent with citation capabilities.

### Chat with it

```bash
punch chat "What are the key differences between Rust and Go for systems programming?"
```

### Or via the API

```bash
# List fighters
curl http://localhost:6660/api/fighters

# Send a message
curl -X POST http://localhost:6660/api/fighters/{fighter_id}/message \
  -H "Content-Type: application/json" \
  -d '{"message": "Explain the actor model in distributed systems"}'
```

## Step 5: Create a Custom Fighter

Fighters are defined by their **manifest** — a JSON object that controls personality, model, and capabilities:

```bash
curl -X POST http://localhost:6660/api/fighters \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "name": "Atlas",
      "description": "Senior architect who thinks in systems",
      "system_prompt": "You are Atlas, a senior systems architect. You think about distributed systems, scalability, and trade-offs. You always consider failure modes. You draw from real-world experience at companies that operate at scale.",
      "model": {
        "provider": "ollama",
        "model": "qwen3:8b",
        "base_url": "http://localhost:11434",
        "max_tokens": 4096,
        "temperature": 0.7
      },
      "weight_class": "heavyweight",
      "capabilities": [{"type": "memory"}]
    }
  }'
```

The fighter now has a persistent identity. Every conversation strengthens its **Creed** — the living document that defines who it is.

## Step 6: Unleash a Gorilla

Gorillas are autonomous background agents that run on schedules without human interaction.

```bash
# List available gorillas
punch gorilla list

# Unleash the Alpha researcher (runs every 6 hours)
punch gorilla unleash alpha

# Check its status
punch gorilla status alpha

# Stop it
punch gorilla cage alpha
```

### Bundled gorillas

| Gorilla         | Schedule     | What it does                                           |
| --------------- | ------------ | ------------------------------------------------------ |
| **Alpha**       | Every 6h     | Deep research with cross-referencing and fact-checking |
| **Ghost**       | Every 30m    | OSINT monitoring, change detection, anomaly analysis   |
| **Prophet**     | Daily        | Probabilistic forecasting with Brier score calibration |
| **Scout Troop** | Every 4h     | Lead generation with ICP-based scoring                 |
| **Swarm**       | Every 3h     | Multi-platform social media content creation           |
| **Brawler**     | Every 2h     | Web automation, form filling, data extraction          |
| **Howler**      | Every 2 days | Short-form video script creation                       |

## Step 7: Fighter-to-Fighter Communication

Fighters can talk to each other — and they remember who they've spoken to:

```bash
# Spawn two fighters
curl -X POST http://localhost:6660/api/fighters \
  -H "Content-Type: application/json" \
  -d '{"manifest": {"name": "Optimist", "description": "Sees opportunity everywhere", "system_prompt": "You are optimistic about technology and its potential to solve problems."}}'

curl -X POST http://localhost:6660/api/fighters \
  -H "Content-Type: application/json" \
  -d '{"manifest": {"name": "Skeptic", "description": "Questions everything", "system_prompt": "You are deeply skeptical about technology hype. You demand evidence and question assumptions."}}'

# Have them debate
curl -X POST http://localhost:6660/api/fighters/{optimist_id}/message-to/{skeptic_id} \
  -H "Content-Type: application/json" \
  -d '{"content": "AI agents will replace 50% of knowledge work within 3 years. Change my mind."}'
```

## Step 8: Form a Troop

Troops coordinate multiple fighters with different strategies:

```bash
# Create a troop with the Pipeline strategy
curl -X POST http://localhost:6660/api/troops \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Review Pipeline",
    "leader": "{architect_id}",
    "members": ["{coder_id}", "{reviewer_id}", "{tester_id}"],
    "strategy": "pipeline"
  }'

# Assign a task — it flows through each member sequentially
curl -X POST http://localhost:6660/api/troops/{troop_id}/tasks \
  -H "Content-Type: application/json" \
  -d '{"task": "Write a rate limiter in Rust, review it for security, then write tests"}'
```

### Available strategies

| Strategy         | How it works                                         |
| ---------------- | ---------------------------------------------------- |
| **Pipeline**     | Output of agent N becomes input to agent N+1         |
| **Broadcast**    | All agents receive the same task, results aggregated |
| **Consensus**    | All agents vote, majority wins                       |
| **LeaderWorker** | Leader decomposes task, workers execute              |
| **RoundRobin**   | Tasks distributed evenly in rotation                 |
| **Specialist**   | Routed to best-matching agent by capability          |

## Step 9: Install Skills from the Marketplace

Skills (called "Moves" in Punch) add domain expertise to your fighters:

```bash
# Search for skills
punch move search "security"

# Install one
punch move install security-auditor

# List what's installed
punch move list

# Security scan a skill before installing
punch move scan security-auditor
```

Punch ships with **103 bundled skills** covering programming languages, frameworks, cloud platforms, business operations, and more.

## Step 10: Connect a Channel

Route fighters to messaging platforms so users can talk to them from Telegram, Discord, Slack, and 23 other platforms:

Add to your `~/.punch/config.toml`:

```toml
[telegram]
bot_token_env = "TELEGRAM_BOT_TOKEN"
webhook_url = "https://yourdomain.com/api/channels/telegram/webhook"
```

Then restart Punch. Your fighter now responds to Telegram messages.

## What to Explore Next

- **Creeds** — Build persistent agent identities: `GET /api/creeds/{name}/render`
- **Workflows** — Define multi-step automation: `POST /api/workflows`
- **Triggers** — Fire actions on events: `POST /api/triggers`
- **Budgets** — Set spending limits per fighter: `PUT /api/budget/fighters/{id}`
- **A2A Protocol** — Delegate to remote agents: `POST /a2a/tasks/send`
- **WASM Plugins** — Extend with WebAssembly: capability `PluginInvoke`
- **P2P Federation** — Connect Punch instances: `punch-wire` protocol
- **Dashboard** — Monitor everything live: `http://localhost:6660/dashboard`

## Configuration Reference

See [`punch.toml.example`](../punch.toml.example) for the full configuration with all options documented.

## Architecture

See [architecture.md](architecture.md) for the internal architecture deep-dive.

## Security

See [security.md](security.md) for the 18-layer security model.
