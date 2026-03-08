//! Session message history repair.
//!
//! Fixes common issues in message histories that can cause LLM API errors:
//! - Orphaned tool results (tool_result with no matching tool_use)
//! - Empty messages
//! - Consecutive same-role messages that should be merged
//! - Tool uses with no corresponding result
//! - Duplicate tool results for the same tool_use_id

use std::collections::HashSet;

use tracing::{debug, info, warn};

use punch_types::{Message, Role, ToolCallResult};

/// Statistics from a repair pass.
#[derive(Debug, Clone, Default)]
pub struct RepairStats {
    /// Number of empty messages removed.
    pub empty_removed: usize,
    /// Number of orphaned tool results removed.
    pub orphaned_results_removed: usize,
    /// Number of synthetic error results inserted for tool_uses without results.
    pub synthetic_results_inserted: usize,
    /// Number of duplicate tool results removed.
    pub duplicate_results_removed: usize,
    /// Number of consecutive same-role message merges performed.
    pub messages_merged: usize,
}

impl RepairStats {
    /// Whether any repairs were made.
    pub fn any_repairs(&self) -> bool {
        self.empty_removed > 0
            || self.orphaned_results_removed > 0
            || self.synthetic_results_inserted > 0
            || self.duplicate_results_removed > 0
            || self.messages_merged > 0
    }
}

impl std::fmt::Display for RepairStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "empty_removed={}, orphaned_results={}, synthetic_inserts={}, duplicates={}, merges={}",
            self.empty_removed,
            self.orphaned_results_removed,
            self.synthetic_results_inserted,
            self.duplicate_results_removed,
            self.messages_merged,
        )
    }
}

/// Run all repair passes on a message history.
///
/// This is idempotent: running repair twice produces the same result.
/// The passes run in a specific order to handle dependencies correctly.
pub fn repair_session(messages: &mut Vec<Message>) -> RepairStats {
    let mut stats = RepairStats::default();

    // Pass 1: Remove empty messages.
    remove_empty_messages(messages, &mut stats);

    // Pass 2: Remove duplicate tool results.
    remove_duplicate_tool_results(messages, &mut stats);

    // Pass 3: Fix orphaned tool results (results with no matching tool_use).
    remove_orphaned_tool_results(messages, &mut stats);

    // Pass 4: Insert synthetic error results for tool_uses with no result.
    insert_synthetic_results(messages, &mut stats);

    // Pass 5: Merge consecutive same-role messages.
    merge_consecutive_same_role(messages, &mut stats);

    if stats.any_repairs() {
        info!(repairs = %stats, "session repair completed");
    } else {
        debug!("session repair: no repairs needed");
    }

    stats
}

/// Remove messages that have no content, no tool calls, and no tool results.
fn remove_empty_messages(messages: &mut Vec<Message>, stats: &mut RepairStats) {
    let before = messages.len();

    messages.retain(|msg| {
        let is_empty =
            msg.content.is_empty() && msg.tool_calls.is_empty() && msg.tool_results.is_empty();
        !is_empty
    });

    let removed = before - messages.len();
    if removed > 0 {
        debug!(count = removed, "removed empty messages");
        stats.empty_removed = removed;
    }
}

/// Remove tool results whose tool_use_id does not match any tool_use in the history.
fn remove_orphaned_tool_results(messages: &mut Vec<Message>, stats: &mut RepairStats) {
    // Collect all tool_use IDs from assistant messages.
    let tool_use_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.tool_calls)
        .map(|tc| tc.id.clone())
        .collect();

    let mut removed = 0;

    for msg in messages.iter_mut() {
        if msg.role == Role::Tool && !msg.tool_results.is_empty() {
            let before = msg.tool_results.len();
            msg.tool_results.retain(|tr| tool_use_ids.contains(&tr.id));
            let delta = before - msg.tool_results.len();
            if delta > 0 {
                warn!(
                    count = delta,
                    "removed orphaned tool results (no matching tool_use)"
                );
                removed += delta;
            }
        }
    }

    stats.orphaned_results_removed = removed;

    // Also remove any Tool messages that now have zero results and no content.
    messages.retain(|msg| {
        if msg.role == Role::Tool {
            !msg.tool_results.is_empty() || !msg.content.is_empty()
        } else {
            true
        }
    });
}

/// Insert synthetic error results for tool_uses that have no corresponding result.
fn insert_synthetic_results(messages: &mut Vec<Message>, stats: &mut RepairStats) {
    // Collect all tool_result IDs.
    let result_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .flat_map(|m| &m.tool_results)
        .map(|tr| tr.id.clone())
        .collect();

    // Find tool_uses that have no result, grouped by their position.
    // We need to insert results AFTER the assistant message that made the call.
    let mut insertions: Vec<(usize, Vec<ToolCallResult>)> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
            let missing: Vec<ToolCallResult> = msg
                .tool_calls
                .iter()
                .filter(|tc| !result_ids.contains(&tc.id))
                .map(|tc| {
                    warn!(
                        tool_use_id = %tc.id,
                        tool_name = %tc.name,
                        "inserting synthetic error result for orphaned tool_use"
                    );
                    ToolCallResult {
                        id: tc.id.clone(),
                        content: format!(
                            "Error: tool execution was interrupted or result was lost (tool: {})",
                            tc.name
                        ),
                        is_error: true,
                    }
                })
                .collect();

            if !missing.is_empty() {
                insertions.push((idx, missing));
            }
        }
    }

    // Insert in reverse order to preserve indices.
    let mut inserted = 0;
    for (idx, results) in insertions.into_iter().rev() {
        let count = results.len();
        inserted += count;

        let tool_msg = Message {
            role: Role::Tool,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_results: results,
            timestamp: chrono::Utc::now(),
        };

        // Insert right after the assistant message.
        let insert_pos = idx + 1;
        if insert_pos <= messages.len() {
            messages.insert(insert_pos, tool_msg);
        } else {
            messages.push(tool_msg);
        }
    }

    stats.synthetic_results_inserted = inserted;
}

/// Remove duplicate tool results (same tool_use_id appearing more than once).
fn remove_duplicate_tool_results(messages: &mut [Message], stats: &mut RepairStats) {
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut removed = 0;

    for msg in messages.iter_mut() {
        if msg.role == Role::Tool && !msg.tool_results.is_empty() {
            let before = msg.tool_results.len();
            msg.tool_results.retain(|tr| seen_ids.insert(tr.id.clone()));
            let delta = before - msg.tool_results.len();
            if delta > 0 {
                debug!(count = delta, "removed duplicate tool results");
                removed += delta;
            }
        }
    }

    stats.duplicate_results_removed = removed;
}

/// Merge consecutive messages with the same role.
///
/// This handles cases where, e.g., two user messages end up adjacent
/// (which some LLM APIs reject).
fn merge_consecutive_same_role(messages: &mut Vec<Message>, stats: &mut RepairStats) {
    if messages.len() < 2 {
        return;
    }

    let mut merged = 0;
    let mut result: Vec<Message> = Vec::with_capacity(messages.len());

    for msg in messages.drain(..) {
        if let Some(last) = result.last_mut() {
            // Only merge User-User or Assistant-Assistant.
            // Tool messages have special structure and should not be merged.
            if last.role == msg.role && (msg.role == Role::User || msg.role == Role::Assistant) {
                // Merge content.
                if !msg.content.is_empty() {
                    if !last.content.is_empty() {
                        last.content.push('\n');
                    }
                    last.content.push_str(&msg.content);
                }
                // Merge tool calls and results.
                last.tool_calls.extend(msg.tool_calls);
                last.tool_results.extend(msg.tool_results);
                // Keep the later timestamp.
                last.timestamp = msg.timestamp;
                merged += 1;
                continue;
            }
        }
        result.push(msg);
    }

    *messages = result;
    stats.messages_merged = merged;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::{Message, Role, ToolCall, ToolCallResult};

    fn user_msg(content: &str) -> Message {
        Message::new(Role::User, content)
    }

    fn assistant_msg(content: &str) -> Message {
        Message::new(Role::Assistant, content)
    }

    fn assistant_with_tool_call(tool_id: &str, tool_name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input: serde_json::json!({}),
            }],
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    fn tool_result_msg(id: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_results: vec![ToolCallResult {
                id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
            timestamp: chrono::Utc::now(),
        }
    }

    fn empty_msg(role: Role) -> Message {
        Message {
            role,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_remove_empty_messages() {
        let mut msgs = vec![
            user_msg("hello"),
            empty_msg(Role::Assistant),
            assistant_msg("world"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.empty_removed, 1);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_remove_orphaned_tool_results() {
        let mut msgs = vec![
            user_msg("hello"),
            assistant_with_tool_call("call_1", "file_read"),
            tool_result_msg("call_1", "file contents"),
            // This tool result has no matching tool_use:
            tool_result_msg("call_999", "orphaned result"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.orphaned_results_removed, 1);
        // The orphaned tool message should be fully removed since it has no results left.
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_insert_synthetic_results() {
        let mut msgs = vec![
            user_msg("do something"),
            assistant_with_tool_call("call_1", "shell_exec"),
            // No tool result for call_1!
            assistant_msg("I ran the command"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.synthetic_results_inserted, 1);

        // Should now have: user, assistant(tool_use), tool(synthetic), assistant
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[2].role, Role::Tool);
        assert!(msgs[2].tool_results[0].is_error);
        assert!(msgs[2].tool_results[0].content.contains("interrupted"));
    }

    #[test]
    fn test_remove_duplicate_tool_results() {
        let mut msgs = vec![
            user_msg("hello"),
            assistant_with_tool_call("call_1", "file_read"),
            tool_result_msg("call_1", "first result"),
            tool_result_msg("call_1", "duplicate result"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.duplicate_results_removed, 1);
    }

    #[test]
    fn test_merge_consecutive_user_messages() {
        let mut msgs = vec![
            user_msg("hello"),
            user_msg("world"),
            assistant_msg("hi there"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.messages_merged, 1);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].content.contains("hello"));
        assert!(msgs[0].content.contains("world"));
    }

    #[test]
    fn test_merge_consecutive_assistant_messages() {
        let mut msgs = vec![
            user_msg("hello"),
            assistant_msg("part 1"),
            assistant_msg("part 2"),
        ];

        let stats = repair_session(&mut msgs);
        assert_eq!(stats.messages_merged, 1);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1].content.contains("part 1"));
        assert!(msgs[1].content.contains("part 2"));
    }

    #[test]
    fn test_no_merge_tool_messages() {
        let mut msgs = vec![
            user_msg("hello"),
            assistant_with_tool_call("call_1", "file_read"),
            tool_result_msg("call_1", "result 1"),
            assistant_with_tool_call("call_2", "file_read"),
            tool_result_msg("call_2", "result 2"),
            assistant_msg("done"),
        ];

        let stats = repair_session(&mut msgs);
        // Tool messages should not be merged, and the assistant messages with
        // tool calls should not be merged with each other.
        assert_eq!(stats.messages_merged, 0);
        assert_eq!(msgs.len(), 6);
    }

    #[test]
    fn test_clean_session_no_repairs() {
        let mut msgs = vec![
            user_msg("hello"),
            assistant_with_tool_call("call_1", "file_read"),
            tool_result_msg("call_1", "result"),
            assistant_msg("done"),
        ];

        let stats = repair_session(&mut msgs);
        assert!(!stats.any_repairs());
        assert_eq!(msgs.len(), 4);
    }

    #[test]
    fn test_idempotent() {
        let mut msgs = vec![
            user_msg("hello"),
            empty_msg(Role::Assistant),
            assistant_with_tool_call("call_1", "file_read"),
            tool_result_msg("call_1", "result"),
            tool_result_msg("call_999", "orphaned"),
            user_msg("follow up"),
            user_msg("more"),
        ];

        let stats1 = repair_session(&mut msgs);
        assert!(stats1.any_repairs());

        let snapshot = msgs.clone();
        let stats2 = repair_session(&mut msgs);
        assert!(!stats2.any_repairs());
        assert_eq!(msgs.len(), snapshot.len());
    }
}
