//! Context window budget management.
//!
//! Tracks estimated token usage and enforces limits to prevent context overflow.
//! Uses a chars/4 heuristic for token estimation (conservative but fast).

use tracing::{debug, info, warn};

use punch_types::{Message, Role, ToolDefinition};

/// Default context window size in tokens.
const DEFAULT_WINDOW_SIZE: usize = 200_000;

/// Threshold (as fraction of window) for moderate trimming.
const MODERATE_TRIM_THRESHOLD: f64 = 0.70;

/// Threshold (as fraction of window) for aggressive trimming.
const AGGRESSIVE_TRIM_THRESHOLD: f64 = 0.90;

/// Messages to keep during moderate trim.
const MODERATE_KEEP_LAST: usize = 10;

/// Messages to keep during aggressive trim.
const AGGRESSIVE_KEEP_LAST: usize = 4;

/// Fraction of window allowed per individual tool result.
const PER_RESULT_CAP_FRACTION: f64 = 0.30;

/// Absolute max fraction for a single tool result.
const SINGLE_RESULT_MAX_FRACTION: f64 = 0.50;

/// Total fraction of window available for all tool results combined.
const TOTAL_TOOL_HEADROOM_FRACTION: f64 = 0.75;

/// Context budget configuration and enforcement.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Maximum tokens in the context window.
    pub window_size: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            window_size: DEFAULT_WINDOW_SIZE,
        }
    }
}

impl ContextBudget {
    /// Create a new context budget with the given window size.
    pub fn new(window_size: usize) -> Self {
        Self { window_size }
    }

    /// Estimate the token count of a set of messages and tool definitions.
    ///
    /// Uses the chars/4 heuristic: each character is roughly 0.25 tokens.
    /// This is conservative (overestimates) which is safer than underestimating.
    pub fn estimate_tokens(&self, messages: &[Message], tools: &[ToolDefinition]) -> usize {
        let mut total_chars: usize = 0;

        for msg in messages {
            total_chars += msg.content.len();
            for tc in &msg.tool_calls {
                total_chars += tc.name.len();
                total_chars += tc.input.to_string().len();
                total_chars += tc.id.len();
            }
            for tr in &msg.tool_results {
                total_chars += tr.content.len();
                total_chars += tr.id.len();
            }
        }

        for tool in tools {
            total_chars += tool.name.len();
            total_chars += tool.description.len();
            total_chars += tool.input_schema.to_string().len();
        }

        // chars / 4 heuristic
        total_chars / 4
    }

    /// Estimate tokens for messages only (no tool definitions).
    pub fn estimate_message_tokens(&self, messages: &[Message]) -> usize {
        self.estimate_tokens(messages, &[])
    }

    /// Maximum chars allowed per individual tool result.
    pub fn per_result_cap(&self) -> usize {
        // Convert from tokens back to chars (* 4)
        ((self.window_size as f64) * PER_RESULT_CAP_FRACTION * 4.0) as usize
    }

    /// Absolute maximum chars for a single tool result.
    pub fn single_result_max(&self) -> usize {
        ((self.window_size as f64) * SINGLE_RESULT_MAX_FRACTION * 4.0) as usize
    }

    /// Total chars available for all tool results combined.
    pub fn total_tool_headroom(&self) -> usize {
        ((self.window_size as f64) * TOTAL_TOOL_HEADROOM_FRACTION * 4.0) as usize
    }

    /// Truncate a tool result string to fit within max_chars.
    ///
    /// If truncation occurs, appends a `[truncated]` marker.
    pub fn truncate_result(text: &str, max_chars: usize) -> String {
        if text.len() <= max_chars {
            return text.to_string();
        }

        // Leave room for the truncation marker
        let marker = "\n\n[truncated — result exceeded context budget]";
        let keep = max_chars.saturating_sub(marker.len());

        // Find a safe char boundary
        let boundary = find_char_boundary(text, keep);

        let mut result = text[..boundary].to_string();
        result.push_str(marker);
        result
    }

    /// Apply the context guard to messages: trim oldest tool results when
    /// total tool result content exceeds headroom.
    ///
    /// Returns the (possibly modified) messages and whether trimming occurred.
    pub fn apply_context_guard(&self, messages: &mut [Message]) -> bool {
        let headroom = self.total_tool_headroom();
        let per_cap = self.per_result_cap();
        let single_max = self.single_result_max();
        let mut trimmed = false;

        // First pass: truncate individual oversized tool results.
        for msg in messages.iter_mut() {
            if msg.role == Role::Tool {
                for tr in msg.tool_results.iter_mut() {
                    let cap = per_cap.min(single_max);
                    if tr.content.len() > cap {
                        debug!(
                            tool_result_id = %tr.id,
                            original_len = tr.content.len(),
                            cap = cap,
                            "truncating oversized tool result"
                        );
                        tr.content = Self::truncate_result(&tr.content, cap);
                        trimmed = true;
                    }
                }
            }
        }

        // Second pass: if total tool result content exceeds headroom,
        // truncate oldest tool results first.
        let total_tool_chars: usize = messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .flat_map(|m| &m.tool_results)
            .map(|tr| tr.content.len())
            .sum();

        if total_tool_chars > headroom {
            debug!(
                total_tool_chars = total_tool_chars,
                headroom = headroom,
                "tool results exceed headroom, trimming oldest"
            );

            // Collect indices of tool messages, oldest first (they're in chronological order).
            let tool_indices: Vec<usize> = messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.role == Role::Tool)
                .map(|(i, _)| i)
                .collect();

            let mut current_total = total_tool_chars;

            // Trim from oldest tool messages until we're under headroom.
            for &idx in &tool_indices {
                if current_total <= headroom {
                    break;
                }
                let msg = &mut messages[idx];
                for tr in msg.tool_results.iter_mut() {
                    if current_total <= headroom {
                        break;
                    }
                    let old_len = tr.content.len();
                    // Aggressively truncate old results to 200 chars.
                    if old_len > 200 {
                        tr.content = Self::truncate_result(&tr.content, 200);
                        current_total -= old_len - tr.content.len();
                        trimmed = true;
                    }
                }
            }
        }

        trimmed
    }

    /// Determine the trim action needed based on current token estimate.
    ///
    /// Returns `None` if no trimming needed, or the trim action to take.
    pub fn check_trim_needed(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Option<TrimAction> {
        let tokens = self.estimate_tokens(messages, tools);
        let ratio = tokens as f64 / self.window_size as f64;

        if ratio > AGGRESSIVE_TRIM_THRESHOLD {
            warn!(
                tokens = tokens,
                window = self.window_size,
                ratio = format!("{:.1}%", ratio * 100.0),
                "context usage critical — aggressive trim needed"
            );
            Some(TrimAction::Aggressive)
        } else if ratio > MODERATE_TRIM_THRESHOLD {
            info!(
                tokens = tokens,
                window = self.window_size,
                ratio = format!("{:.1}%", ratio * 100.0),
                "context usage high — moderate trim needed"
            );
            Some(TrimAction::Moderate)
        } else {
            None
        }
    }

    /// Apply a trim action to messages. Returns the trimmed messages.
    ///
    /// Preserves the first message (usually the user's initial prompt) and
    /// system markers, then keeps the last N messages.
    pub fn apply_trim(&self, messages: &mut Vec<Message>, action: TrimAction) {
        let keep = match action {
            TrimAction::Moderate => MODERATE_KEEP_LAST,
            TrimAction::Aggressive => AGGRESSIVE_KEEP_LAST,
        };

        if messages.len() <= keep {
            return;
        }

        let original_len = messages.len();

        // Always keep the first message (user's initial prompt).
        let first = messages[0].clone();
        let tail: Vec<Message> = messages
            .iter()
            .rev()
            .take(keep)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        messages.clear();
        messages.push(first);

        // For aggressive trim, insert a summary marker.
        if matches!(action, TrimAction::Aggressive) {
            messages.push(Message::new(
                Role::System,
                format!(
                    "[Context trimmed: {} earlier messages removed to stay within context window. \
                     Conversation may reference prior context that is no longer visible.]",
                    original_len - 1 - tail.len()
                ),
            ));
        }

        messages.extend(tail);

        info!(
            original = original_len,
            trimmed_to = messages.len(),
            action = ?action,
            "context window trimmed"
        );
    }
}

/// What kind of trim to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimAction {
    /// Keep last 10 messages (70-90% usage).
    Moderate,
    /// Keep last 4 messages + insert summary marker (>90% usage).
    Aggressive,
}

/// Find a valid UTF-8 char boundary at or before `pos`.
fn find_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut boundary = pos;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::{Message, Role, ToolCallResult, ToolCategory, ToolDefinition};

    fn make_message(role: Role, content: &str) -> Message {
        Message::new(role, content)
    }

    fn make_tool_message(results: Vec<ToolCallResult>) -> Message {
        Message {
            role: Role::Tool,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_results: results,
            timestamp: chrono::Utc::now(),
            content_parts: Vec::new(),
        }
    }

    fn make_tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            category: ToolCategory::FileSystem,
        }
    }

    #[test]
    fn test_estimate_tokens_basic() {
        let budget = ContextBudget::new(200_000);
        // 400 chars / 4 = 100 tokens
        let msg = make_message(Role::User, &"x".repeat(400));
        let tokens = budget.estimate_tokens(&[msg], &[]);
        assert_eq!(tokens, 100);
    }

    #[test]
    fn test_estimate_tokens_with_tools() {
        let budget = ContextBudget::new(200_000);
        let msgs = vec![make_message(Role::User, "hello")];
        let tools = vec![make_tool_def("file_read")];
        let tokens_with = budget.estimate_tokens(&msgs, &tools);
        let tokens_without = budget.estimate_tokens(&msgs, &[]);
        assert!(tokens_with > tokens_without);
    }

    #[test]
    fn test_truncate_result_no_truncation() {
        let text = "short text";
        let result = ContextBudget::truncate_result(text, 100);
        assert_eq!(result, text);
    }

    #[test]
    fn test_truncate_result_with_truncation() {
        let text = "a".repeat(1000);
        let result = ContextBudget::truncate_result(&text, 200);
        assert!(result.len() <= 200 + 50); // some slack for marker
        assert!(result.contains("[truncated"));
    }

    #[test]
    fn test_per_result_cap() {
        let budget = ContextBudget::new(200_000);
        // 30% of 200K tokens * 4 chars/token = 240K chars
        assert_eq!(budget.per_result_cap(), 240_000);
    }

    #[test]
    fn test_single_result_max() {
        let budget = ContextBudget::new(200_000);
        // 50% of 200K tokens * 4 chars/token = 400K chars
        assert_eq!(budget.single_result_max(), 400_000);
    }

    #[test]
    fn test_total_tool_headroom() {
        let budget = ContextBudget::new(200_000);
        // 75% of 200K tokens * 4 chars/token = 600K chars
        assert_eq!(budget.total_tool_headroom(), 600_000);
    }

    #[test]
    fn test_check_trim_not_needed() {
        let budget = ContextBudget::new(200_000);
        // Small message, well under 70%
        let msgs = vec![make_message(Role::User, "hello")];
        assert!(budget.check_trim_needed(&msgs, &[]).is_none());
    }

    #[test]
    fn test_check_trim_moderate() {
        let budget = ContextBudget::new(1_000); // 1K token window
        // 750 tokens * 4 chars = 3000 chars -> 75% of window
        let msgs = vec![make_message(Role::User, &"x".repeat(3000))];
        let action = budget.check_trim_needed(&msgs, &[]);
        assert_eq!(action, Some(TrimAction::Moderate));
    }

    #[test]
    fn test_check_trim_aggressive() {
        let budget = ContextBudget::new(1_000); // 1K token window
        // 950 tokens * 4 chars = 3800 chars -> 95% of window
        let msgs = vec![make_message(Role::User, &"x".repeat(3800))];
        let action = budget.check_trim_needed(&msgs, &[]);
        assert_eq!(action, Some(TrimAction::Aggressive));
    }

    #[test]
    fn test_apply_trim_moderate() {
        let budget = ContextBudget::new(200_000);
        let mut msgs: Vec<Message> = (0..20)
            .map(|i| make_message(Role::User, &format!("message {}", i)))
            .collect();

        budget.apply_trim(&mut msgs, TrimAction::Moderate);

        // First message + last 10 = 11
        assert_eq!(msgs.len(), 11);
        assert!(msgs[0].content.contains("message 0"));
        assert!(msgs.last().unwrap().content.contains("message 19"));
    }

    #[test]
    fn test_apply_trim_aggressive() {
        let budget = ContextBudget::new(200_000);
        let mut msgs: Vec<Message> = (0..20)
            .map(|i| make_message(Role::User, &format!("message {}", i)))
            .collect();

        budget.apply_trim(&mut msgs, TrimAction::Aggressive);

        // First message + summary marker + last 4 = 6
        assert_eq!(msgs.len(), 6);
        assert!(msgs[0].content.contains("message 0"));
        assert!(msgs[1].role == Role::System);
        assert!(msgs[1].content.contains("Context trimmed"));
        assert!(msgs.last().unwrap().content.contains("message 19"));
    }

    #[test]
    fn test_apply_context_guard_truncates_oversized() {
        // Use a small window so the cap is small
        let budget = ContextBudget::new(100); // 100 tokens
        // per_result_cap = 0.30 * 100 * 4 = 120 chars
        let big_result = "x".repeat(500);
        let mut msgs = vec![make_tool_message(vec![ToolCallResult {
            id: "call_1".into(),
            content: big_result,
            is_error: false,
            image: None,
        }])];

        let trimmed = budget.apply_context_guard(&mut msgs);
        assert!(trimmed);
        assert!(msgs[0].tool_results[0].content.len() < 500);
    }

    #[test]
    fn test_apply_context_guard_no_change_when_small() {
        let budget = ContextBudget::new(200_000);
        let mut msgs = vec![make_tool_message(vec![ToolCallResult {
            id: "call_1".into(),
            content: "small result".into(),
            is_error: false,
            image: None,
        }])];

        let trimmed = budget.apply_context_guard(&mut msgs);
        assert!(!trimmed);
        assert_eq!(msgs[0].tool_results[0].content, "small result");
    }

    #[test]
    fn test_find_char_boundary_ascii() {
        let s = "hello world";
        assert_eq!(find_char_boundary(s, 5), 5);
    }

    #[test]
    fn test_find_char_boundary_multibyte() {
        let s = "hello 世界";
        // '世' starts at byte 6, is 3 bytes. Asking for boundary at 7 should back up to 6.
        let boundary = find_char_boundary(s, 7);
        assert!(s.is_char_boundary(boundary));
        assert!(boundary <= 7);
    }

    // -----------------------------------------------------------------------
    // Additional context budget tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_default_context_budget() {
        let budget = ContextBudget::default();
        assert_eq!(budget.window_size, 200_000);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        let budget = ContextBudget::new(200_000);
        let tokens = budget.estimate_tokens(&[], &[]);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn test_estimate_tokens_with_tool_calls() {
        let budget = ContextBudget::new(200_000);
        let msg = Message {
            role: Role::Assistant,
            content: "thinking".into(),
            tool_calls: vec![punch_types::ToolCall {
                id: "call_1".into(),
                name: "file_read".into(),
                input: serde_json::json!({"path": "/tmp/test.txt"}),
            }],
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
            content_parts: Vec::new(),
        };
        let tokens = budget.estimate_tokens(&[msg], &[]);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_tokens_with_tool_results() {
        let budget = ContextBudget::new(200_000);
        let msg = Message {
            role: Role::Tool,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_results: vec![punch_types::ToolCallResult {
                id: "call_1".into(),
                content: "x".repeat(400),
                is_error: false,
                image: None,
            }],
            timestamp: chrono::Utc::now(),
            content_parts: Vec::new(),
        };
        let tokens = budget.estimate_tokens(&[msg], &[]);
        assert!(tokens >= 100); // 400+ chars / 4
    }

    #[test]
    fn test_estimate_message_tokens() {
        let budget = ContextBudget::new(200_000);
        let msgs = vec![make_message(Role::User, &"x".repeat(800))];
        let tokens = budget.estimate_message_tokens(&msgs);
        assert_eq!(tokens, 200); // 800 / 4
    }

    #[test]
    fn test_per_result_cap_custom_window() {
        let budget = ContextBudget::new(100_000);
        // 30% of 100K * 4 = 120K chars
        assert_eq!(budget.per_result_cap(), 120_000);
    }

    #[test]
    fn test_single_result_max_custom_window() {
        let budget = ContextBudget::new(100_000);
        // 50% of 100K * 4 = 200K chars
        assert_eq!(budget.single_result_max(), 200_000);
    }

    #[test]
    fn test_truncate_result_exact_boundary() {
        let text = "a".repeat(100);
        let result = ContextBudget::truncate_result(&text, 100);
        // Should not truncate when exactly at boundary
        assert_eq!(result, text);
    }

    #[test]
    fn test_truncate_result_one_over() {
        let text = "a".repeat(101);
        let result = ContextBudget::truncate_result(&text, 100);
        assert!(result.len() <= 150); // some slack for marker
        assert!(result.contains("[truncated"));
    }

    #[test]
    fn test_apply_trim_fewer_than_keep() {
        let budget = ContextBudget::new(200_000);
        let mut msgs: Vec<Message> = (0..3)
            .map(|i| make_message(Role::User, &format!("msg {}", i)))
            .collect();

        budget.apply_trim(&mut msgs, TrimAction::Moderate);
        // Should not trim if fewer messages than keep count
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_apply_trim_preserves_first_message() {
        let budget = ContextBudget::new(200_000);
        let mut msgs: Vec<Message> = (0..30)
            .map(|i| make_message(Role::User, &format!("msg {}", i)))
            .collect();

        budget.apply_trim(&mut msgs, TrimAction::Moderate);
        assert!(msgs[0].content.contains("msg 0"));
    }

    #[test]
    fn test_apply_trim_aggressive_inserts_marker() {
        let budget = ContextBudget::new(200_000);
        let mut msgs: Vec<Message> = (0..15)
            .map(|i| make_message(Role::User, &format!("msg {}", i)))
            .collect();

        budget.apply_trim(&mut msgs, TrimAction::Aggressive);
        // Should have: first + marker + last 4
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[1].role, Role::System);
        assert!(msgs[1].content.contains("Context trimmed"));
    }

    #[test]
    fn test_check_trim_below_moderate() {
        let budget = ContextBudget::new(10_000);
        // 6000 tokens * 4 chars = 24000 chars -> 60% of window, below 70%
        let msgs = vec![make_message(Role::User, &"x".repeat(24_000))];
        assert!(budget.check_trim_needed(&msgs, &[]).is_none());
    }

    #[test]
    fn test_apply_context_guard_total_headroom_exceeded() {
        // Very small window to trigger total headroom exceeded path
        let budget = ContextBudget::new(10);
        let big_result = "y".repeat(500);
        let mut msgs = vec![
            make_tool_message(vec![ToolCallResult {
                id: "c1".into(),
                content: big_result.clone(),
                is_error: false,
                image: None,
            }]),
            make_tool_message(vec![ToolCallResult {
                id: "c2".into(),
                content: big_result,
                is_error: false,
                image: None,
            }]),
        ];

        let trimmed = budget.apply_context_guard(&mut msgs);
        assert!(trimmed);
    }

    #[test]
    fn test_find_char_boundary_at_end() {
        let s = "hello";
        assert_eq!(find_char_boundary(s, 100), s.len());
    }

    #[test]
    fn test_find_char_boundary_at_zero() {
        let s = "hello";
        assert_eq!(find_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_trim_action_equality() {
        assert_eq!(TrimAction::Moderate, TrimAction::Moderate);
        assert_eq!(TrimAction::Aggressive, TrimAction::Aggressive);
        assert_ne!(TrimAction::Moderate, TrimAction::Aggressive);
    }
}
