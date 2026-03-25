# Channels — Talk to Your Agents From Anywhere

Channels connect your Punch fighters to the messaging platforms your team already uses. Instead of curling an API, users message a Telegram bot, ping a Slack app, or talk to a Discord bot — and a fighter responds with full MCP tool access.

## Why Channels?

Agents behind an API are useless to most people. Your sales team won't open a terminal. Your ops team won't write curl commands at 2 AM. Channels put fighters in Telegram, Slack, Discord — where your team already lives.

Every channel message flows through Punch's security gateway (signature verification, allowlists, rate limiting) before reaching a fighter. The fighter responds using the same MCP tools it has in the API — no capability loss.

## Quick Start

```bash
punch channel setup telegram
```

The wizard handles five things:

1. Walks you through creating a bot on the platform
2. Collects your bot token and user ID
3. Generates a webhook secret for signature verification
4. Sets up your tunnel / public URL (one-time, shared by all channels)
5. Registers the webhook with the platform API

Adding a second channel reuses the same tunnel — no extra infra:

```bash
punch channel setup slack    # reuses the same tunnel URL
punch channel setup discord  # same again
```

Total time: under 2 minutes for the first channel, under 1 minute for each additional.

## The Key Concept: One Tunnel, Many Channels

The tunnel is per-machine, not per-channel. One tunnel exposes `localhost:6660` and every channel shares the same base URL with different webhook paths:

```
https://your-tunnel-url.com/api/channels/telegram/webhook
https://your-tunnel-url.com/api/channels/slack/events
https://your-tunnel-url.com/api/channels/discord/webhook
```

The tunnel URL is saved to `~/.punch/config.toml` the first time you run the wizard. Every subsequent `punch channel setup <platform>` reuses it automatically.

## Choosing Your Tunnel Mode

The wizard asks how you want to expose your machine. Pick based on your use case:

| Mode                     | What it does                                 | Best for                           |
| ------------------------ | -------------------------------------------- | ---------------------------------- |
| **1. Local testing**     | Quick tunnel — random URL, dies on restart   | "Does my bot work?"                |
| **2. Persistent access** | Named tunnel — stable URL, survives restarts | Checking on agents from your phone |
| **3. Own URL**           | Paste any URL you control                    | VPS, ngrok, existing infra         |

### Option 1: Local Testing (Quick Tunnel)

Spins up a temporary Cloudflare Tunnel. Free, no account needed, works in seconds. The URL is random and changes every restart — webhooks break when you restart.

Good for: verifying the bot responds, testing message flow, demos.

```
  > Select [1/2/3]: 1
  [+] Starting tunnel to 127.0.0.1:6660...
  [+] Tunnel URL: https://random-words.trycloudflare.com
```

If you don't have `cloudflared` installed, the wizard tells you how. You can also use `npx cloudflared tunnel --url http://127.0.0.1:6660` and paste the URL.

**Installing cloudflared:**

```bash
# macOS
brew install cloudflare/cloudflare/cloudflared

# Linux (Debian/Ubuntu)
curl -L https://pkg.cloudflare.com/cloudflared-stable-linux-amd64.deb -o cloudflared.deb
sudo dpkg -i cloudflared.deb

# Other platforms
# https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/
```

### Option 2: Persistent Access (Named Tunnel)

A stable URL that survives restarts. Requires a free Cloudflare account and a domain on Cloudflare. This is how you message your fighter from your phone while your machine is at home.

```bash
# One-time setup (the wizard walks you through this)
cloudflared tunnel login
cloudflared tunnel create punch
cloudflared tunnel route dns punch channels.yourdomain.com
cloudflared tunnel run punch
```

Your webhook URL becomes: `https://channels.yourdomain.com/api/channels/telegram/webhook`

### Option 3: Own URL

You already have a public URL — a VPS, ngrok, a reverse proxy. Just paste it. The wizard registers webhooks against it.

## The Complete Flow

```
User
  |
  v
Telegram / Slack / Discord
  |
  v (HTTPS webhook)
Cloudflare Tunnel (or your reverse proxy)
  |
  v (localhost:6660)
Punch Arena (HTTP API)
  |
  v
Security Gateway
  |-- Signature verification (HMAC / platform-specific)
  |-- User allowlist check
  +-- Rate limit enforcement (per-user, per-minute)
  |
  v
Persistent Router
  |
  v
Fighter (with MCP tools, memory, creed)
  |
  v
Response
  |
  v (platform API)
Telegram / Slack / Discord
  |
  v
User
```

## Security Architecture

The tunnel is just a pipe — it doesn't add or remove security. All security is inside Punch's middleware:

### 1. Signature Verification

Every incoming webhook is verified against the platform's signing mechanism:

- **Telegram**: `X-Telegram-Bot-Api-Secret-Token` header checked against your webhook secret
- **Slack**: `X-Slack-Signature` HMAC-SHA256 verification with signing secret
- **Discord**: Ed25519 signature verification on the interaction payload

Unsigned or incorrectly signed requests are rejected with 403.

### 2. User Allowlist

The `allowed_user_ids` field restricts which platform users can reach your fighters. An empty list means open access (useful for public bots, logs a security warning).

```toml
allowed_user_ids = ["111222333", "444555666"]
```

### 3. Rate Limiting

Per-user rate limiting prevents abuse. Default: 20 messages per minute per user.

```toml
rate_limit_per_user = 20
```

### 4. Auth Isolation

Each channel gets its own token and secret. Compromising one channel doesn't expose others. Tokens are stored as environment variable references (`token_env`), never as plaintext in config.

### What about the tunnel URL being public?

For **quick tunnels**: the URL is random and ephemeral. Nobody knows it exists unless you register it as a webhook. It's effectively security through obscurity + the four layers above.

For **named tunnels / production**: Cloudflare handles TLS termination and DDoS protection automatically. Combined with Punch's signature verification + allowlist, this is the standard security model used by every webhook-based integration.

No Cloudflare Worker or API gateway is needed. The platforms themselves (Telegram, Slack, Discord) all sign their webhook payloads, and Punch verifies those signatures.

## Platform Guides

### Telegram

The fastest platform to set up. One bot token, one webhook, done.

```bash
punch channel setup telegram
```

**What you need:**

- A Telegram account
- 2 minutes with @BotFather

**What the wizard does:**

1. Guides you through @BotFather bot creation
2. Collects your bot token
3. Asks for your Telegram user ID (for allowlisting)

**Finding your Telegram user ID:** Message [@userinfobot](https://t.me/userinfobot) on Telegram — it replies with your numeric ID (e.g., `8514018060`). Use this number, NOT your @username.

4. Generates a webhook secret
5. Registers the webhook via the Telegram Bot API

**Manual setup (without wizard):**

Add to `~/.punch/config.toml`:

```toml
[tunnel]
base_url = "https://your-tunnel-url.com"
mode = "named"   # or "quick" or "manual"

[channels.telegram]
channel_type = "telegram"
token_env = "TELEGRAM_BOT_TOKEN"
webhook_secret_env = "TELEGRAM_WEBHOOK_SECRET"
allowed_user_ids = ["YOUR_USER_ID"]
rate_limit_per_user = 20
```

Add to `~/.punch/.env`:

```
TELEGRAM_BOT_TOKEN=123456:ABC-DEF...
TELEGRAM_WEBHOOK_SECRET=your_hex_secret
```

Register the webhook:

```bash
curl -X POST "https://api.telegram.org/bot<TOKEN>/setWebhook" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://your-tunnel-url.com/api/channels/telegram/webhook", "secret_token": "your_hex_secret"}'
```

### Slack

Slack requires an app with Event Subscriptions enabled.

```bash
punch channel setup slack
```

**What you need:**

- A Slack workspace you can install apps to
- Access to [api.slack.com/apps](https://api.slack.com/apps)

**Setup steps:**

1. Create a new Slack app at the API portal
2. Add bot scopes: `chat:write`, `channels:read`, `channels:history`
3. Install the app to your workspace
4. Copy the Bot User OAuth Token (`xoxb-...`)
5. Enable Event Subscriptions with your webhook URL

**Config:**

```toml
[channels.slack]
channel_type = "slack"
token_env = "SLACK_BOT_TOKEN"
webhook_secret_env = "SLACK_WEBHOOK_SECRET"
allowed_user_ids = []
rate_limit_per_user = 20
```

### Discord

Discord bots require an application with the Message Content Intent enabled.

```bash
punch channel setup discord
```

**What you need:**

- A Discord server you manage
- Access to the [Developer Portal](https://discord.com/developers/applications)

**Setup steps:**

1. Create a new application
2. Add a bot and enable Message Content Intent
3. Copy the bot token
4. Generate an OAuth2 invite URL with `Send Messages` permission
5. Invite the bot to your server

**Config:**

```toml
[channels.discord]
channel_type = "discord"
token_env = "DISCORD_BOT_TOKEN"
webhook_secret_env = "DISCORD_WEBHOOK_SECRET"
allowed_user_ids = []
rate_limit_per_user = 20
```

### WhatsApp

WhatsApp requires a Meta Business account and the WhatsApp Cloud API.

```bash
punch channel setup whatsapp
```

**What you need:**

- A Meta Business account
- A WhatsApp Business phone number
- Access to the Meta Developer Portal

**Setup steps:**

1. Create an app at developers.facebook.com
2. Add the WhatsApp product
3. Get a test phone number (or verify your own)
4. Copy the access token and phone number ID
5. The wizard registers the webhook automatically

**Config:**

```toml
[channels.whatsapp]
channel_type = "whatsapp"
token_env = "WHATSAPP_ACCESS_TOKEN"
webhook_secret_env = "WHATSAPP_VERIFY_TOKEN"
allowed_user_ids = []
rate_limit_per_user = 20
```

**Free tier:** 1,000 service conversations per month.

### SMS (Twilio)

SMS via Twilio — your fighter responds to text messages.

**What you need:**

- A Twilio account (free trial available)
- A Twilio phone number (~$1/month)

**Config:**

```toml
[channels.sms]
channel_type = "sms"
token_env = "TWILIO_AUTH_TOKEN"
webhook_secret_env = "TWILIO_ACCOUNT_SID"
allowed_user_ids = []
rate_limit_per_user = 20
```

## Configuration Reference

Full annotated config:

```toml
# Tunnel — shared by ALL channels. Set up once, reused automatically.
[tunnel]
base_url = "https://your-tunnel-url.com"
mode = "named"   # "quick" | "named" | "manual"

# Each channel gets its own section
[channels.telegram]
channel_type = "telegram"
token_env = "TELEGRAM_BOT_TOKEN"
webhook_secret_env = "TELEGRAM_WEBHOOK_SECRET"
allowed_user_ids = ["111222333"]
rate_limit_per_user = 20

[channels.slack]
channel_type = "slack"
token_env = "SLACK_BOT_TOKEN"
webhook_secret_env = "SLACK_WEBHOOK_SECRET"
allowed_user_ids = []
rate_limit_per_user = 20
```

Environment variables in `~/.punch/.env`:

```bash
# Telegram
TELEGRAM_BOT_TOKEN=123456789:ABCdef...
TELEGRAM_WEBHOOK_SECRET=a1b2c3d4e5f6...

# Slack
SLACK_BOT_TOKEN=xoxb-...
SLACK_WEBHOOK_SECRET=...

# Discord
DISCORD_BOT_TOKEN=...
DISCORD_WEBHOOK_SECRET=...
```

## Managing Your Setup

Everything the wizard creates lives in two files on your machine:

| File                   | What's in it                                                         |
| ---------------------- | -------------------------------------------------------------------- |
| `~/.punch/config.toml` | Tunnel URL, channel configs (platform type, allowlists, rate limits) |
| `~/.punch/.env`        | Bot tokens, webhook secrets (never stored in config)                 |

### View current tunnel

```bash
punch channel tunnel
```

Shows the base URL, mode, and all webhook paths that use it.

### Change the tunnel URL

```bash
# Switch to a new URL
punch channel tunnel set https://new-url.com

# Specify the mode explicitly
punch channel tunnel set https://channels.mysite.com --mode named
```

After changing the URL, you'll need to re-register webhooks with each platform. The easiest way is to re-run setup:

```bash
punch channel setup telegram   # detects new tunnel URL, re-registers webhook
```

### Remove the tunnel

```bash
punch channel tunnel remove
```

This removes the `[tunnel]` section from config. Webhook registrations with platforms stay active until you deregister them on the platform side.

### Remove a channel

```bash
punch channel remove telegram
```

This removes:

- The `[channels.telegram]` section from `~/.punch/config.toml`
- All related env vars (`TELEGRAM_BOT_TOKEN`, `TELEGRAM_WEBHOOK_SECRET`) from `~/.punch/.env`

The webhook registration with Telegram stays active. It'll start returning errors once Punch stops accepting those webhooks.

### Edit config manually

You can always edit the files directly:

```bash
# Open config in your editor
punch config edit
# or just:
nano ~/.punch/config.toml
nano ~/.punch/.env
```

## Troubleshooting

### Tunnel drops after a few minutes

Quick tunnels (trycloudflare.com) are best-effort. For reliability, use option 2 (named tunnel) or option 3 (your own URL).

### Quick tunnel URL changed and webhooks are broken

Re-run the wizard — it will start a new tunnel and re-register the webhook:

```bash
punch channel setup telegram
```

Or for a permanent fix, switch to a named tunnel (option 2).

### Webhook returns 403 Forbidden

- **Signature mismatch**: Verify your `webhook_secret_env` matches what the platform expects. Regenerate if unsure.
- **Allowlist rejection**: Check `allowed_user_ids` — your platform user ID might not be listed. Use an empty list `[]` to disable allowlisting.

### Rate limited (429 Too Many Requests)

Increase `rate_limit_per_user` in config, or set to `0` for unlimited. Default is 20 messages/minute.

### No fighters available

The daemon auto-spawns a default fighter on startup. Just make sure it's running:

```bash
punch start
```

### Webhook registered but no responses

1. Check the daemon is running: `punch status`
2. Check channel status: `punch channel status telegram`
3. Test the webhook locally: `punch channel test telegram`
4. Check logs: the daemon logs all incoming webhook requests at `debug` level

### Adding a second channel

Just run the wizard again. It reuses your existing tunnel automatically:

```bash
punch channel setup slack    # sees [tunnel] in config, skips tunnel setup
```

### Model ignores tools / says "I can't"

Use `gpt-4.1-mini` or better. Nano and lite models don't reliably invoke tools. Edit `[default_model]` in `~/.punch/config.toml`.

### Port 6660 already in use

Kill the existing process:

```bash
kill $(lsof -t -i :6660)
```

Then restart with `punch start`.

### Platform-specific bot token errors

- **Telegram**: Token format is `123456789:ABCdefGHI...`. Get it from @BotFather.
- **Slack**: Token starts with `xoxb-`. Get it from your app's OAuth page.
- **Discord**: Token is a long base64 string. Get it from the Bot section of your app.
