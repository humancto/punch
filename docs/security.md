# Punch Security Architecture

> Punch implements **18 security layers** — the most comprehensive security model of any agent framework.

## Overview

Security in an agent system is fundamentally different from traditional application security. Agents can execute arbitrary tools, access external systems, and operate autonomously for extended periods. A single vulnerability can cascade through an entire fleet of agents.

Punch takes a defense-in-depth approach with 18 interlocking security layers, each designed to contain failures and prevent escalation.

## The 18 Security Layers

### Layer 1: Capability-Based Access Control

Every action an agent can perform requires an explicit `CapabilityGrant`. Agents operate on the principle of least privilege — they can only use moves (tools) that are explicitly granted in their manifest.

```rust
pub struct CapabilityGrant {
    pub capability: Capability,
    pub scope: GrantScope,      // Global, PerBout, TimeLimited
    pub granted_by: String,     // Who authorized this grant
    pub expires_at: Option<DateTime<Utc>>,
}
```

**Key properties:**

- Grants are additive — agents start with zero capabilities
- Grants can be scoped to specific bouts, time windows, or global
- Grants can be revoked at runtime without restarting the agent
- The Ring validates every tool invocation against the fighter's grants

### Layer 2: Per-Agent Sandboxing

Each fighter operates in an isolated capability space. Fighter A's capabilities are completely invisible to Fighter B, even if they share the same Ring instance.

- Fighters cannot enumerate other fighters' capabilities
- Fighters cannot invoke moves through another fighter's grants
- Memory access is scoped to the fighter's own bouts

### Layer 3: API Key Vault

Secrets are never stored in configuration files. Instead, configuration references environment variable names:

```toml
[default_model]
api_key_env = "ANTHROPIC_API_KEY"   # References $ANTHROPIC_API_KEY
```

**Key properties:**

- Keys are resolved from environment variables at startup
- Keys are stored in memory using `Zeroize`-capable containers
- Keys are never logged, serialized, or included in error messages
- Keys are cleared from memory when no longer needed

### Layer 4: Ed25519 Request Signing

All inter-component messages within Punch are cryptographically signed using Ed25519:

- The Ring signs all events published to the Event Bus
- The Arena validates signatures on all incoming API requests
- Channel adapters sign messages before forwarding to the Ring
- Replay attacks are prevented via nonce + timestamp validation

### Layer 5: AES-256-GCM Encryption at Rest

The memory substrate (SQLite database) is encrypted using AES-256-GCM authenticated encryption:

- All conversation data, entity graphs, and bout histories are encrypted
- The encryption key is derived from the master key (see Layer 6)
- GCM provides both confidentiality and integrity — tampered data is detected
- Encryption is transparent to the application layer

### Layer 6: Argon2id Key Derivation

The master encryption key is derived using Argon2id, a memory-hard key derivation function:

- Resistant to GPU/ASIC-based brute force attacks
- Parameters tuned for security: high memory cost, multiple iterations
- Salt is unique per installation
- The derived key is used for AES-256-GCM (Layer 5) and token signing

### Layer 7: Rate Limiting and Quotas

The Scheduler enforces per-agent rate limits and resource quotas:

- **Requests per minute (RPM):** Limits how frequently an agent can call LLMs
- **Tokens per minute (TPM):** Limits token consumption per agent
- **Concurrent requests:** Limits parallel LLM calls per agent
- Exceeded quotas transition the fighter to `Resting` status with a retry-after hint
- Gorillas have separate quota pools from fighters

### Layer 8: Input Sanitization

All user inputs are sanitized before reaching the LLM:

- Prompt injection detection and mitigation
- Control character stripping
- Unicode normalization to prevent homoglyph attacks
- Maximum input length enforcement
- Template literal escaping for system prompts

### Layer 9: Output Filtering

Agent outputs are filtered before being returned to users:

- Sensitive data pattern detection (API keys, tokens, passwords, SSNs, credit cards)
- PII detection and optional redaction
- Configurable output content policies
- Blocked pattern lists for domain-specific filtering

### Layer 10: Audit Logging

Every agent action is logged with full traceability:

- All tool invocations logged with input/output hashes
- All LLM calls logged with model, token counts, and latency
- All lifecycle events (spawn, kill, unleash, cage) logged with actor identity
- Structured JSON logging via `tracing` with correlation IDs
- Audit logs are append-only and tamper-evident

### Layer 11: TLS-Only External Communications

All outbound network connections use TLS via `rustls`:

- No plaintext HTTP allowed for LLM API calls
- Certificate validation enforced (no `danger_accept_invalid_certs`)
- TLS 1.2 minimum, TLS 1.3 preferred
- System certificate store used by default, custom CA bundles supported

### Layer 12: CORS and Origin Validation

The Arena API validates request origins:

- Configurable CORS allowed origins (no wildcard in production)
- Strict origin checking on WebSocket upgrade requests
- Referer validation as a secondary check
- Pre-flight request handling via `tower-http` CORS middleware

### Layer 13: Token Rotation

Session tokens are automatically rotated on a configurable schedule:

- Default rotation every 24 hours
- Old tokens remain valid for a grace period during rotation
- Rotation can be triggered manually via the Arena API
- Compromised tokens can be immediately invalidated

### Layer 14: Memory Decay

Old conversation data automatically decays, reducing the exposure window of sensitive information:

```
relevance(t) = initial_score * e^(-decay_rate * t)
```

- Memories with scores below a threshold are eligible for garbage collection
- Reduces the risk of historical data exfiltration
- Configurable decay rate per installation
- Critical memories can be pinned to prevent decay

### Layer 15: Zeroize Secrets

All cryptographic material is zeroized from memory when dropped:

- Ed25519 private keys implement `Zeroize + ZeroizeOnDrop`
- AES-256 keys implement `Zeroize + ZeroizeOnDrop`
- Argon2 derived material is zeroized immediately after use
- API key strings are stored in `Zeroizing<String>` containers
- Prevents memory dump attacks from recovering secrets

### Layer 16: Resource Limits

Per-agent resource limits prevent denial-of-service and runaway costs:

- **CPU time limits:** Maximum wall-clock time per tool execution
- **Memory limits:** Maximum heap allocation per agent
- **Network limits:** Bandwidth and connection count limits per agent
- **Storage limits:** Maximum database rows and file sizes per agent
- Limits are enforced by the Ring's Scheduler and the OS-level process controls

### Layer 17: Gorilla Containment Zones

Each gorilla runs in an isolated execution environment — a **containment zone** — with its own capability boundary:

- Gorillas cannot access other gorillas' memory or state
- Gorillas cannot invoke moves outside their manifest's `moves_required` list
- Background task handles are isolated — one gorilla cannot abort or inspect another
- File system access is scoped to the gorilla's designated workspace directory
- Network access is restricted to explicitly whitelisted domains
- A compromised gorilla cannot move laterally to other gorillas or fighters

**Why this matters:** Gorillas operate autonomously on schedules. A compromised gorilla running 24/7 is far more dangerous than a compromised interactive fighter. Containment zones ensure that even a fully compromised gorilla cannot affect the rest of the system.

### Layer 18: Cross-Troop Privilege Firewall

Troops (coordinated agent squads) cannot escalate each other's capabilities:

- A troop's effective capabilities are the **intersection** (not union) of its members' grants
- Troop A cannot access Troop B's shared context or coordination channels
- Adding a highly-privileged agent to a troop does not grant those privileges to other troop members
- Troop dissolution immediately revokes all troop-scoped grants
- Privilege boundaries are enforced at the Ring level, not the troop level

**Why this matters:** Without this firewall, an attacker could create a troop containing a low-privilege agent and a high-privilege agent, then use the troop coordination mechanism to launder requests through the high-privilege agent. The firewall prevents this by ensuring that troop membership never increases any individual agent's capabilities.

## Security Comparison

| Security Feature             | Punch   | OpenFang | CrewAI  | AutoGen | LangGraph |
| ---------------------------- | ------- | -------- | ------- | ------- | --------- |
| Capability-based access      | Yes     | Yes      | No      | No      | No        |
| Per-agent sandboxing         | Yes     | Yes      | No      | No      | No        |
| Secret vault                 | Yes     | Yes      | Partial | No      | No        |
| Request signing              | Yes     | Yes      | No      | No      | No        |
| Encryption at rest           | Yes     | Yes      | No      | No      | No        |
| Memory-hard KDF              | Yes     | Yes      | No      | No      | No        |
| Rate limiting                | Yes     | Yes      | No      | No      | Partial   |
| Input sanitization           | Yes     | Yes      | Partial | Partial | Partial   |
| Output filtering             | Yes     | Yes      | No      | No      | No        |
| Audit logging                | Yes     | Yes      | Partial | Partial | Partial   |
| TLS enforcement              | Yes     | Yes      | Partial | Partial | Partial   |
| CORS validation              | Yes     | Yes      | N/A     | N/A     | Partial   |
| Token rotation               | Yes     | Yes      | No      | No      | No        |
| Memory decay                 | Yes     | Yes      | No      | No      | No        |
| Zeroize secrets              | Yes     | Yes      | No      | No      | No        |
| Resource limits              | Yes     | Yes      | No      | No      | No        |
| **Gorilla containment**      | **Yes** | No       | No      | No      | No        |
| **Troop privilege firewall** | **Yes** | No       | No      | No      | No        |
| **Total layers**             | **18**  | **16**   | **3**   | **2**   | **4**     |

## Threat Model

Punch's security architecture is designed to protect against these threat categories:

### External Threats

- **Prompt injection:** Mitigated by input sanitization (Layer 8) and capability restrictions (Layer 1)
- **API key theft:** Mitigated by secret vault (Layer 3), zeroize (Layer 15), and encryption at rest (Layer 5)
- **Man-in-the-middle:** Mitigated by TLS enforcement (Layer 11) and request signing (Layer 4)
- **Unauthorized API access:** Mitigated by CORS (Layer 12), signing (Layer 4), and token rotation (Layer 13)

### Internal Threats

- **Compromised agent lateral movement:** Mitigated by per-agent sandboxing (Layer 2) and gorilla containment (Layer 17)
- **Privilege escalation via troops:** Mitigated by cross-troop privilege firewall (Layer 18)
- **Resource exhaustion:** Mitigated by rate limiting (Layer 7) and resource limits (Layer 16)
- **Data exfiltration from memory:** Mitigated by encryption at rest (Layer 5), memory decay (Layer 14), and output filtering (Layer 9)

### Operational Threats

- **Runaway autonomous agents:** Mitigated by gorilla containment (Layer 17), resource limits (Layer 16), and audit logging (Layer 10)
- **Configuration drift:** Mitigated by capability-based access (Layer 1) — agents cannot grant themselves new capabilities
- **Post-compromise forensics:** Enabled by audit logging (Layer 10) with structured, tamper-evident logs

## Reporting Security Issues

If you discover a security vulnerability in Punch, please report it responsibly:

1. **Do not** open a public GitHub issue
2. Email `security@humancto.com` with details
3. Include steps to reproduce, impact assessment, and suggested fix if possible
4. We will acknowledge within 48 hours and provide a fix timeline within 7 days
