//! The core agent execution loop.
//!
//! `run_fighter_loop` is the heart of the Punch runtime. It orchestrates the
//! conversation between the user, the LLM, and the tools (moves), persisting
//! messages to the memory substrate and enforcing loop guards.
//!
//! ## Production features
//!
//! - **Context window management**: Tracks estimated token count and trims
//!   messages when approaching the context limit.
//! - **Session repair**: Fixes orphaned tool results, empty messages,
//!   duplicate results, and missing results on startup and after errors.
//! - **Error recovery**: Handles empty responses, MaxTokens continuation,
//!   and per-tool timeouts.
//! - **Loop guard**: Graduated response (Allow → Warn → Block → CircuitBreak)
//!   with ping-pong detection and poll-tool relaxation.

use std::sync::Arc;

use serde::Deserialize as SerdeDeserialize;
use tracing::{debug, error, info, instrument, warn};

use dashmap::DashMap;
use punch_memory::{BoutId, MemorySubstrate};
use punch_types::{
    capability::Capability, AgentCoordinator, FighterId, FighterManifest, Message, PolicyEngine,
    PunchError, PunchResult, Role, SandboxEnforcer, ShellBleedDetector, ToolCallResult,
    ToolDefinition,
};

use crate::mcp::McpClient;

use crate::context_budget::ContextBudget;
use crate::driver::{CompletionRequest, LlmDriver, StopReason, TokenUsage};
use crate::guard::{GuardConfig, LoopGuard, LoopGuardVerdict};
use crate::session_repair;
use crate::tool_executor::{self, ToolExecutionContext};

/// Maximum number of MaxTokens continuations before giving up.
const MAX_CONTINUATION_LOOPS: usize = 5;

/// Default per-tool timeout in seconds.
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;

/// Parameters for the fighter loop.
pub struct FighterLoopParams {
    /// The fighter's manifest (identity, model config, system prompt, capabilities).
    pub manifest: FighterManifest,
    /// The user's message to process.
    pub user_message: String,
    /// The bout (session) ID.
    pub bout_id: BoutId,
    /// The fighter's unique ID.
    pub fighter_id: FighterId,
    /// Shared memory substrate for persistence.
    pub memory: Arc<MemorySubstrate>,
    /// The LLM driver to use for completions.
    pub driver: Arc<dyn LlmDriver>,
    /// Tools available for this fighter to use.
    pub available_tools: Vec<ToolDefinition>,
    /// Maximum loop iterations before forced termination (default: 50).
    pub max_iterations: Option<usize>,
    /// Context window size in tokens (default: 200K).
    pub context_window: Option<usize>,
    /// Per-tool timeout in seconds (default: 120).
    pub tool_timeout_secs: Option<u64>,
    /// Optional agent coordinator for inter-agent tools.
    pub coordinator: Option<Arc<dyn AgentCoordinator>>,
    /// Optional policy engine for approval-gated tool execution.
    /// When present, the referee checks every move before the fighter can throw it.
    pub approval_engine: Option<Arc<PolicyEngine>>,
    /// Optional subprocess sandbox (containment ring) for shell and filesystem tools.
    /// When present, commands are validated and environments are sanitized before execution.
    pub sandbox: Option<Arc<SandboxEnforcer>>,
    /// Active MCP server clients shared across fighters.
    /// When present, MCP tools are available for dispatch.
    pub mcp_clients: Option<Arc<DashMap<String, Arc<McpClient>>>>,
}

/// Result of a completed fighter loop run.
#[derive(Debug, Clone)]
pub struct FighterLoopResult {
    /// The final text response from the fighter.
    pub response: String,
    /// Cumulative token usage across all LLM calls in this run.
    pub usage: TokenUsage,
    /// Number of loop iterations performed.
    pub iterations: usize,
    /// Number of individual tool calls executed.
    pub tool_calls_made: usize,
}

/// Run the fighter loop: the core agent execution engine.
///
/// This function:
/// 1. Loads message history from the bout and repairs it
/// 2. Recalls relevant memories
/// 3. Builds the system prompt with context
/// 4. Applies context budget management before each LLM call
/// 5. Calls the LLM with available tools
/// 6. If the LLM requests tool use, executes tools and loops
/// 7. Handles empty responses, MaxTokens continuation, and errors
/// 8. Enforces loop guards against runaway iterations
#[instrument(
    skip(params),
    fields(
        fighter = %params.fighter_id,
        bout = %params.bout_id,
        fighter_name = %params.manifest.name,
    )
)]
pub async fn run_fighter_loop(params: FighterLoopParams) -> PunchResult<FighterLoopResult> {
    let max_iterations = params.max_iterations.unwrap_or(50);
    let context_window = params.context_window.unwrap_or(200_000);
    let tool_timeout = params
        .tool_timeout_secs
        .unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS);

    let budget = ContextBudget::new(context_window);
    let mut guard = LoopGuard::with_config(GuardConfig {
        max_iterations,
        ..Default::default()
    });
    let mut total_usage = TokenUsage::default();
    let mut tool_calls_made: usize = 0;
    let mut continuation_count: usize = 0;

    // 1. Load message history and repair.
    let mut messages = params.memory.load_messages(&params.bout_id).await?;
    debug!(history_len = messages.len(), "loaded bout message history");

    // Run session repair on loaded history.
    let repair_stats = session_repair::repair_session(&mut messages);
    if repair_stats.any_repairs() {
        info!(repairs = %repair_stats, "repaired loaded message history");
    }

    // 2. Append the user's new message and persist it.
    let user_msg = Message::new(Role::User, &params.user_message);
    params
        .memory
        .save_message(&params.bout_id, &user_msg)
        .await?;
    messages.push(user_msg);

    // 3. Recall relevant memories and build an enriched system prompt.
    let system_prompt =
        build_system_prompt(&params.manifest, &params.fighter_id, &params.memory).await;

    // Build the tool execution context.
    let tool_context = ToolExecutionContext {
        working_dir: std::env::current_dir().unwrap_or_default(),
        fighter_id: params.fighter_id,
        memory: Arc::clone(&params.memory),
        coordinator: params.coordinator.clone(),
        approval_engine: params.approval_engine.clone(),
        sandbox: params.sandbox.clone(),
        bleed_detector: Some(Arc::new(ShellBleedDetector::new())),
        browser_pool: None,
        plugin_registry: None,
        mcp_clients: params.mcp_clients.clone(),
        automation_backend: if params.manifest.capabilities.iter().any(|c| {
            matches!(
                c,
                Capability::SystemAutomation
                    | Capability::UiAutomation(_)
                    | Capability::AppIntegration(_)
            )
        }) {
            crate::automation::create_backend()
                .ok()
                .map(Arc::from)
        } else {
            None
        },
    };

    // 4. Main loop.
    loop {
        // --- Context Budget: check and trim before LLM call ---
        if let Some(trim_action) = budget.check_trim_needed(&messages, &params.available_tools) {
            budget.apply_trim(&mut messages, trim_action);

            // Re-run session repair after trimming (may create orphans).
            let post_trim_repair = session_repair::repair_session(&mut messages);
            if post_trim_repair.any_repairs() {
                debug!(repairs = %post_trim_repair, "repaired after context trim");
            }
        }

        // Apply context guard (truncate oversized tool results).
        budget.apply_context_guard(&mut messages);

        // Build the completion request.
        let request = CompletionRequest {
            model: params.manifest.model.model.clone(),
            messages: messages.clone(),
            tools: params.available_tools.clone(),
            max_tokens: params.manifest.model.max_tokens.unwrap_or(
                // Reasoning models (Qwen, DeepSeek) use thinking tokens internally,
                // so they need a much higher default to leave room for visible output.
                // The thinking budget can easily consume 2000-4000 tokens alone.
                match params.manifest.model.provider {
                    punch_types::Provider::Ollama => 16384,
                    _ => 4096,
                },
            ),
            temperature: params.manifest.model.temperature,
            system_prompt: Some(system_prompt.clone()),
        };

        // Call the LLM.
        let completion = match params.driver.complete(request).await {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "LLM completion failed");
                return Err(e);
            }
        };
        total_usage.accumulate(&completion.usage);

        debug!(
            stop_reason = ?completion.stop_reason,
            input_tokens = completion.usage.input_tokens,
            output_tokens = completion.usage.output_tokens,
            tool_calls = completion.message.tool_calls.len(),
            "LLM completion received"
        );

        match completion.stop_reason {
            StopReason::EndTurn => {
                // --- Empty response handling ---
                if completion.message.content.is_empty() && completion.message.tool_calls.is_empty()
                {
                    if guard.iterations() == 0 {
                        // Empty response on iteration 0: one-shot retry.
                        warn!("empty response on first iteration, retrying once");
                        guard.record_iteration();
                        continue;
                    }

                    // Empty response after tool use: insert fallback.
                    let has_prior_tools = messages.iter().any(|m| m.role == Role::Tool);

                    if has_prior_tools {
                        warn!("empty response after tool use, inserting fallback");
                        let fallback_msg = Message::new(
                            Role::Assistant,
                            "I completed the requested operations. The tool results above \
                             contain the output.",
                        );
                        params
                            .memory
                            .save_message(&params.bout_id, &fallback_msg)
                            .await?;
                        messages.push(fallback_msg.clone());

                        return Ok(FighterLoopResult {
                            response: fallback_msg.content,
                            usage: total_usage,
                            iterations: guard.iterations(),
                            tool_calls_made,
                        });
                    }
                }

                // The fighter is done. Save and return the response.
                params
                    .memory
                    .save_message(&params.bout_id, &completion.message)
                    .await?;
                messages.push(completion.message.clone());

                let response = completion.message.content.clone();

                info!(
                    iterations = guard.iterations(),
                    tool_calls = tool_calls_made,
                    total_tokens = total_usage.total(),
                    "fighter loop complete"
                );

                // --- CREED EVOLUTION ---
                // Update the creed with bout statistics after completion.
                if let Ok(Some(mut creed)) = params
                    .memory
                    .load_creed_by_name(&params.manifest.name)
                    .await
                {
                    creed.record_bout();
                    creed.record_messages(guard.iterations() as u64 + 1); // +1 for user msg
                    // Bind to current fighter instance
                    creed.fighter_id = Some(params.fighter_id);

                    // --- HEARTBEAT MARK ---
                    // Mark due heartbeat tasks as checked now that the bout is complete.
                    let due_indices: Vec<usize> = creed
                        .heartbeat
                        .iter()
                        .enumerate()
                        .filter(|(_, h)| {
                            if !h.active {
                                return false;
                            }
                            let now = chrono::Utc::now();
                            match h.cadence.as_str() {
                                "every_bout" => true,
                                "on_wake" => h.last_checked.is_none(),
                                "hourly" => h
                                    .last_checked
                                    .is_none_or(|t| (now - t) > chrono::Duration::hours(1)),
                                "daily" => h
                                    .last_checked
                                    .is_none_or(|t| (now - t) > chrono::Duration::hours(24)),
                                _ => false,
                            }
                        })
                        .map(|(i, _)| i)
                        .collect();
                    for idx in due_indices {
                        creed.mark_heartbeat_checked(idx);
                    }

                    if let Err(e) = params.memory.save_creed(&creed).await {
                        warn!(error = %e, "failed to update creed after bout");
                    } else {
                        debug!(fighter = %params.manifest.name, bout_count = creed.bout_count, "creed evolved");
                    }
                }

                // Spawn async reflection to extract learned behaviors from the bout.
                // This runs in the background and does not block the response.
                {
                    let driver = Arc::clone(&params.driver);
                    let memory = Arc::clone(&params.memory);
                    let model = params.manifest.model.model.clone();
                    let fighter_name = params.manifest.name.clone();
                    let reflection_messages = messages.clone();
                    tokio::spawn(async move {
                        reflect_on_bout(driver, memory, model, fighter_name, reflection_messages)
                            .await;
                    });
                }

                return Ok(FighterLoopResult {
                    response,
                    usage: total_usage,
                    iterations: guard.iterations(),
                    tool_calls_made,
                });
            }

            StopReason::MaxTokens => {
                // --- MaxTokens continuation ---
                params
                    .memory
                    .save_message(&params.bout_id, &completion.message)
                    .await?;
                messages.push(completion.message.clone());

                continuation_count += 1;

                if continuation_count > MAX_CONTINUATION_LOOPS {
                    warn!(
                        continuation_count = continuation_count,
                        "max continuation loops exceeded, returning partial response"
                    );
                    return Ok(FighterLoopResult {
                        response: completion.message.content,
                        usage: total_usage,
                        iterations: guard.iterations(),
                        tool_calls_made,
                    });
                }

                info!(
                    continuation = continuation_count,
                    max = MAX_CONTINUATION_LOOPS,
                    "MaxTokens hit, appending continuation prompt"
                );

                // Append a user message asking to continue.
                let continue_msg =
                    Message::new(Role::User, "Please continue from where you left off.");
                params
                    .memory
                    .save_message(&params.bout_id, &continue_msg)
                    .await?;
                messages.push(continue_msg);

                guard.record_iteration();
                continue;
            }

            StopReason::ToolUse => {
                // Reset continuation count since we got a real tool use.
                continuation_count = 0;

                // Check the loop guard before executing tools.
                let verdict = guard.record_tool_calls(&completion.message.tool_calls);
                match verdict {
                    LoopGuardVerdict::Break(reason) => {
                        warn!(reason = %reason, "loop guard triggered");

                        // Save the assistant message, then return with a guard message.
                        params
                            .memory
                            .save_message(&params.bout_id, &completion.message)
                            .await?;
                        messages.push(completion.message.clone());

                        let guard_response = format!(
                            "{}\n\n[Loop terminated: {}]",
                            completion.message.content, reason
                        );

                        return Ok(FighterLoopResult {
                            response: guard_response,
                            usage: total_usage,
                            iterations: guard.iterations(),
                            tool_calls_made,
                        });
                    }
                    LoopGuardVerdict::Continue => {}
                }

                // Save the assistant message (with tool calls).
                params
                    .memory
                    .save_message(&params.bout_id, &completion.message)
                    .await?;
                messages.push(completion.message.clone());

                // Execute each tool call with per-tool timeout.
                let mut tool_results = Vec::new();

                for tc in &completion.message.tool_calls {
                    debug!(tool = %tc.name, id = %tc.id, "executing tool call");

                    // Check per-call guard verdict.
                    let call_verdict = guard.evaluate_call(tc);
                    if let crate::guard::GuardVerdict::Block(reason) = &call_verdict {
                        warn!(tool = %tc.name, reason = %reason, "tool call blocked by guard");
                        tool_results.push(ToolCallResult {
                            id: tc.id.clone(),
                            content: format!("Error: {}", reason),
                            is_error: true,
                        });
                        tool_calls_made += 1;
                        continue;
                    }

                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(tool_timeout),
                        tool_executor::execute_tool(
                            &tc.name,
                            &tc.input,
                            &params.manifest.capabilities,
                            &tool_context,
                        ),
                    )
                    .await;

                    let tool_call_result = match result {
                        Ok(Ok(tool_result)) => {
                            let content = if tool_result.success {
                                tool_result.output.to_string()
                            } else {
                                tool_result
                                    .error
                                    .unwrap_or_else(|| "tool execution failed".to_string())
                            };

                            // Record outcome for future blocking.
                            guard.record_outcome(tc, &content);

                            // Truncate result if it exceeds the per-result cap.
                            let cap = budget.per_result_cap().min(budget.single_result_max());
                            let content = if content.len() > cap {
                                debug!(
                                    tool = %tc.name,
                                    original_len = content.len(),
                                    cap = cap,
                                    "truncating tool result"
                                );
                                ContextBudget::truncate_result(&content, cap)
                            } else {
                                content
                            };

                            ToolCallResult {
                                id: tc.id.clone(),
                                content,
                                is_error: !tool_result.success,
                            }
                        }
                        Ok(Err(e)) => {
                            error!(tool = %tc.name, error = %e, "tool execution error");
                            ToolCallResult {
                                id: tc.id.clone(),
                                content: format!("Error: {}", e),
                                is_error: true,
                            }
                        }
                        Err(_) => {
                            error!(
                                tool = %tc.name,
                                timeout_secs = tool_timeout,
                                "tool execution timed out"
                            );
                            ToolCallResult {
                                id: tc.id.clone(),
                                content: format!(
                                    "Error: tool '{}' timed out after {}s",
                                    tc.name, tool_timeout
                                ),
                                is_error: true,
                            }
                        }
                    };

                    tool_results.push(tool_call_result);
                    tool_calls_made += 1;
                }

                // Create and save the tool results message.
                let tool_msg = Message {
                    role: Role::Tool,
                    content: String::new(),
                    tool_calls: Vec::new(),
                    tool_results,
                    timestamp: chrono::Utc::now(),
                };

                params
                    .memory
                    .save_message(&params.bout_id, &tool_msg)
                    .await?;
                messages.push(tool_msg);

                // Continue the loop -- call the LLM again with tool results.
            }

            StopReason::Error => {
                error!("LLM returned error stop reason");
                return Err(PunchError::Provider {
                    provider: params.manifest.model.provider.to_string(),
                    message: "model returned an error".to_string(),
                });
            }
        }
    }
}

/// Build an enriched system prompt by combining the fighter's base system
/// prompt with recalled memories.
async fn build_system_prompt(
    manifest: &FighterManifest,
    fighter_id: &FighterId,
    memory: &MemorySubstrate,
) -> String {
    let mut prompt = manifest.system_prompt.clone();

    // --- CREED INJECTION ---
    // Load the fighter's creed (consciousness layer) if one exists.
    // The creed is tied to fighter NAME so it persists across respawns.
    match memory.load_creed_by_name(&manifest.name).await {
        Ok(Some(creed)) => {
            prompt.push_str("\n\n");
            prompt.push_str(&creed.render());

            // --- HEARTBEAT INJECTION ---
            // Check for due heartbeat tasks and inject them into the prompt.
            let due_tasks = creed.due_heartbeat_tasks();
            if !due_tasks.is_empty() {
                prompt.push_str("\n\n## HEARTBEAT — Due Tasks\n");
                prompt.push_str(
                    "The following proactive tasks are due. Address them briefly before responding to the user:\n",
                );
                for task in &due_tasks {
                    prompt.push_str(&format!("- {}\n", task.task));
                }
            }
        }
        Ok(None) => {
            // No creed defined — fighter runs without consciousness layer.
        }
        Err(e) => {
            warn!(error = %e, "failed to load creed for fighter");
        }
    }

    // --- SKILL INJECTION ---
    // Load markdown-based skills from workspace, user, and bundled directories.
    {
        let workspace_skills = std::path::Path::new("./skills");
        let user_skills = std::env::var("HOME")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".punch").join("skills"));
        // Bundled skills ship in the binary's directory
        let bundled_skills = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("skills")));

        let skills = punch_skills::load_all_skills(
            Some(workspace_skills),
            user_skills.as_deref(),
            bundled_skills.as_deref(),
        );

        if !skills.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&punch_skills::render_skills_prompt(&skills));
        }
    }

    // Try to recall recent/relevant memories.
    match memory.recall_memories(fighter_id, "", 10).await {
        Ok(memories) if !memories.is_empty() => {
            prompt.push_str("\n\n## Recalled Memories\n");
            for mem in &memories {
                prompt.push_str(&format!(
                    "- **{}**: {} (confidence: {:.0}%)\n",
                    mem.key,
                    mem.value,
                    mem.confidence * 100.0
                ));
            }
        }
        Ok(_) => {
            // No memories to inject.
        }
        Err(e) => {
            warn!(error = %e, "failed to recall memories for system prompt");
        }
    }

    prompt
}

/// A single learned behavior extracted from post-bout reflection.
#[derive(Debug, SerdeDeserialize)]
struct ReflectionItem {
    observation: String,
    confidence: f64,
}

/// Post-bout reflection output from the LLM.
#[derive(Debug, SerdeDeserialize)]
struct ReflectionOutput {
    behaviors: Vec<ReflectionItem>,
    #[serde(default)]
    interaction_quality: Option<f64>,
}

/// Reflect on a completed bout to extract learned behaviors.
///
/// Makes a lightweight LLM call asking the model to extract insights from
/// the conversation. Updates the creed with new learned behaviors and
/// adjusts the user relationship trust based on interaction quality.
async fn reflect_on_bout(
    driver: Arc<dyn LlmDriver>,
    memory: Arc<MemorySubstrate>,
    model: String,
    fighter_name: String,
    messages: Vec<Message>,
) {
    // Only use the last 20 messages to keep the reflection call small
    let recent: Vec<Message> = messages.into_iter().rev().take(20).rev().collect();

    let reflection_prompt = r#"You just completed a conversation. Reflect on it and extract learned behaviors.

Respond ONLY with valid JSON (no markdown fences, no commentary):
{
  "behaviors": [
    {"observation": "what you learned", "confidence": 0.0-1.0}
  ],
  "interaction_quality": 0.0-1.0
}

Rules:
- Extract 0-3 genuinely new insights about the user, effective patterns, or self-improvement notes
- confidence: 0.5 = uncertain, 0.9 = very confident
- interaction_quality: how productive/positive was this interaction (0.5 = neutral, 0.9 = great)
- If nothing notable was learned, return: {"behaviors": [], "interaction_quality": 0.7}
- DO NOT restate your directives or identity as learned behaviors"#;

    let request = CompletionRequest {
        model,
        messages: recent,
        tools: vec![],
        max_tokens: 512,
        temperature: Some(0.3),
        system_prompt: Some(reflection_prompt.to_string()),
    };

    let response = match driver.complete(request).await {
        Ok(resp) => resp,
        Err(e) => {
            debug!(error = %e, fighter = %fighter_name, "reflection LLM call failed (non-critical)");
            return;
        }
    };

    let content = response.message.content.trim().to_string();

    // Try to parse JSON, stripping markdown fences if present
    let json_str = if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            &content[start..=end]
        } else {
            &content
        }
    } else {
        &content
    };

    let output: ReflectionOutput = match serde_json::from_str(json_str) {
        Ok(o) => o,
        Err(e) => {
            debug!(error = %e, fighter = %fighter_name, "failed to parse reflection JSON (non-critical)");
            return;
        }
    };

    // Load creed, apply changes, save
    let mut creed = match memory.load_creed_by_name(&fighter_name).await {
        Ok(Some(c)) => c,
        _ => return,
    };

    // Apply confidence decay to existing behaviors
    creed.decay_learned_behaviors(0.01, 0.3);

    // Learn new behaviors
    for item in &output.behaviors {
        if !item.observation.is_empty() {
            creed.learn(&item.observation, item.confidence.clamp(0.0, 1.0));
        }
    }

    // Prune to max 20 behaviors
    creed.prune_learned_behaviors(20);

    // Update user relationship trust based on interaction quality
    if let Some(quality) = output.interaction_quality {
        let quality = quality.clamp(0.0, 1.0);
        if let Some(rel) = creed
            .relationships
            .iter_mut()
            .find(|r| r.entity_type == "user")
        {
            rel.trust = (rel.trust * 0.9 + quality * 0.1).clamp(0.0, 1.0);
            rel.interaction_count += 1;
        } else {
            creed.relationships.push(punch_types::Relationship {
                entity: "user".to_string(),
                entity_type: "user".to_string(),
                nature: "operator".to_string(),
                trust: quality,
                interaction_count: 1,
                notes: format!(
                    "First interaction: {}",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
                ),
            });
        }
    }

    if let Err(e) = memory.save_creed(&creed).await {
        warn!(error = %e, fighter = %fighter_name, "failed to save creed after reflection");
    } else {
        info!(
            fighter = %fighter_name,
            new_behaviors = output.behaviors.len(),
            total_behaviors = creed.learned_behaviors.len(),
            "creed evolved via post-bout reflection"
        );
    }
}
