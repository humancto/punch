//! The core agent execution loop.
//!
//! `run_fighter_loop` is the heart of the Punch runtime. It orchestrates the
//! conversation between the user, the LLM, and the tools (moves), persisting
//! messages to the memory substrate and enforcing loop guards.

use std::sync::Arc;

use tracing::{debug, error, info, instrument, warn};

use punch_memory::{BoutId, MemorySubstrate};
use punch_types::{
    FighterId, FighterManifest, Message, PunchError, PunchResult, Role, ToolCallResult,
    ToolDefinition,
};

use crate::driver::{CompletionRequest, LlmDriver, StopReason, TokenUsage};
use crate::guard::{LoopGuard, LoopGuardVerdict};
use crate::tool_executor::{self, ToolExecutionContext};

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
/// 1. Loads message history from the bout
/// 2. Recalls relevant memories
/// 3. Builds the system prompt with context
/// 4. Calls the LLM with available tools
/// 5. If the LLM requests tool use, executes tools and loops
/// 6. If the LLM ends its turn, saves messages and returns
/// 7. Enforces loop guards against runaway iterations
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
    let mut guard = LoopGuard::new(max_iterations, 3);
    let mut total_usage = TokenUsage::default();
    let mut tool_calls_made: usize = 0;

    // 1. Load message history.
    let mut messages = params.memory.load_messages(&params.bout_id).await?;
    debug!(
        history_len = messages.len(),
        "loaded bout message history"
    );

    // 2. Append the user's new message and persist it.
    let user_msg = Message::new(Role::User, &params.user_message);
    params
        .memory
        .save_message(&params.bout_id, &user_msg)
        .await?;
    messages.push(user_msg);

    // 3. Recall relevant memories and build an enriched system prompt.
    let system_prompt = build_system_prompt(
        &params.manifest,
        &params.fighter_id,
        &params.memory,
    )
    .await;

    // Build the tool execution context.
    let tool_context = ToolExecutionContext {
        working_dir: std::env::current_dir().unwrap_or_default(),
        fighter_id: params.fighter_id,
        memory: Arc::clone(&params.memory),
    };

    // 4. Main loop.
    loop {
        // Build the completion request.
        let request = CompletionRequest {
            model: params.manifest.model.model.clone(),
            messages: messages.clone(),
            tools: params.available_tools.clone(),
            max_tokens: params.manifest.model.max_tokens.unwrap_or(4096),
            temperature: params.manifest.model.temperature,
            system_prompt: Some(system_prompt.clone()),
        };

        // Call the LLM.
        let completion = params.driver.complete(request).await?;
        total_usage.accumulate(&completion.usage);

        debug!(
            stop_reason = ?completion.stop_reason,
            input_tokens = completion.usage.input_tokens,
            output_tokens = completion.usage.output_tokens,
            tool_calls = completion.message.tool_calls.len(),
            "LLM completion received"
        );

        match completion.stop_reason {
            StopReason::EndTurn | StopReason::MaxTokens => {
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

                return Ok(FighterLoopResult {
                    response,
                    usage: total_usage,
                    iterations: guard.iterations(),
                    tool_calls_made,
                });
            }

            StopReason::ToolUse => {
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

                // Execute each tool call.
                let mut tool_results = Vec::new();

                for tc in &completion.message.tool_calls {
                    debug!(tool = %tc.name, id = %tc.id, "executing tool call");

                    let result = tool_executor::execute_tool(
                        &tc.name,
                        &tc.input,
                        &params.manifest.capabilities,
                        &tool_context,
                    )
                    .await;

                    let tool_call_result = match result {
                        Ok(tool_result) => {
                            let content = if tool_result.success {
                                tool_result.output.to_string()
                            } else {
                                tool_result
                                    .error
                                    .unwrap_or_else(|| "tool execution failed".to_string())
                            };

                            ToolCallResult {
                                id: tc.id.clone(),
                                content,
                                is_error: !tool_result.success,
                            }
                        }
                        Err(e) => {
                            error!(tool = %tc.name, error = %e, "tool execution error");
                            ToolCallResult {
                                id: tc.id.clone(),
                                content: format!("Error: {}", e),
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
