//! Patch application engine — combo patches and move corrections.
//!
//! Provides a unified diff parser, applicator, validator, and generator so agents
//! can land precise code changes without rewriting entire files. Think of it as a
//! carefully choreographed combo: each hunk is a move in the sequence, and the
//! engine ensures every hit connects cleanly.

use crate::error::{PunchError, PunchResult};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single line in a patch hunk — context, removal, or addition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatchLine {
    /// Unchanged context line (space prefix in unified diff).
    Context(String),
    /// Removed line (- prefix in unified diff).
    Remove(String),
    /// Added line (+ prefix in unified diff).
    Add(String),
}

/// A contiguous region of changes within a file patch — one move in the combo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchHunk {
    /// Starting line number in the original file (1-based).
    pub old_start: usize,
    /// Number of lines from the original covered by this hunk.
    pub old_count: usize,
    /// Starting line number in the new file (1-based).
    pub new_start: usize,
    /// Number of lines in the new version covered by this hunk.
    pub new_count: usize,
    /// The diff lines that make up this hunk.
    pub lines: Vec<PatchLine>,
}

/// A patch for a single file — the full combo sequence for one target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePatch {
    /// Path of the original file.
    pub old_path: String,
    /// Path of the new file (may differ for renames).
    pub new_path: String,
    /// The ordered hunks (moves) that compose this patch.
    pub hunks: Vec<PatchHunk>,
    /// Whether this patch creates a brand-new file.
    pub is_new_file: bool,
    /// Whether this patch deletes the file entirely.
    pub is_deleted: bool,
}

/// A collection of file patches — a full combo chain across the codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSet {
    /// Individual file patches in this set.
    pub patches: Vec<FilePatch>,
    /// Optional description of what this patch set accomplishes.
    pub description: Option<String>,
}

/// A detected conflict when validating a patch against file content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchConflict {
    /// Index of the hunk that conflicts.
    pub hunk_index: usize,
    /// The line content the patch expected to find.
    pub expected_line: String,
    /// The line content actually present in the file.
    pub actual_line: String,
    /// The line number (1-based) in the original where the conflict was found.
    pub line_number: usize,
    /// What kind of conflict was detected.
    pub conflict_type: ConflictType,
}

/// Classification of a patch conflict — how badly the move missed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictType {
    /// A context line does not match the original content.
    ContextMismatch,
    /// A line expected for removal was not found at the expected position.
    LineNotFound,
    /// The hunk could be applied at a different offset (signed line count).
    OffsetApplied(i32),
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a unified diff string into a `PatchSet` — decode the combo notation.
///
/// Handles standard unified diff format with `---`/`+++` file headers and
/// `@@ -old_start,old_count +new_start,new_count @@` hunk headers.
pub fn parse_unified_diff(diff_text: &str) -> PunchResult<PatchSet> {
    let lines: Vec<&str> = diff_text.lines().collect();

    if lines.is_empty() {
        return Ok(PatchSet {
            patches: Vec::new(),
            description: None,
        });
    }

    let mut patches = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        // Skip blank lines and any leading text that isn't a file header.
        if !lines[i].starts_with("--- ") {
            // Check for "diff --git" lines — skip them.
            i += 1;
            continue;
        }

        // Parse file header pair: --- and +++
        let old_header = lines[i];
        i += 1;
        if i >= lines.len() || !lines[i].starts_with("+++ ") {
            return Err(PunchError::Tool {
                tool: "patch".into(),
                message: format!("expected '+++ ' header after '--- ' at line {}", i),
            });
        }
        let new_header = lines[i];
        i += 1;

        let old_path = parse_file_path(old_header, "--- ");
        let new_path = parse_file_path(new_header, "+++ ");

        let is_new_file = old_path == "/dev/null";
        let is_deleted = new_path == "/dev/null";

        let mut hunks = Vec::new();

        // Parse hunks for this file.
        while i < lines.len() && lines[i].starts_with("@@ ") {
            let (hunk, consumed) = parse_hunk(&lines[i..])?;
            hunks.push(hunk);
            i += consumed;
        }

        patches.push(FilePatch {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            hunks,
            is_new_file,
            is_deleted,
        });
    }

    Ok(PatchSet {
        patches,
        description: None,
    })
}

/// Extract the file path from a `--- ` or `+++ ` header line.
fn parse_file_path<'a>(line: &'a str, prefix: &str) -> &'a str {
    let path = &line[prefix.len()..];
    // Strip the a/ or b/ prefix if present.
    if let Some(stripped) = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")) {
        stripped
    } else {
        path
    }
}

/// Parse a single hunk starting from an `@@ ... @@` header.
/// Returns the parsed hunk and the number of lines consumed.
fn parse_hunk(lines: &[&str]) -> PunchResult<(PatchHunk, usize)> {
    let header = lines[0];
    let (old_start, old_count, new_start, new_count) = parse_hunk_header(header)?;

    let mut patch_lines = Vec::new();
    let mut i = 1;

    while i < lines.len() {
        let line = lines[i];

        // Stop at a new hunk header or a new file header.
        if line.starts_with("@@ ") || line.starts_with("--- ") || line.starts_with("diff ") {
            break;
        }

        // Handle "\ No newline at end of file" — skip it.
        if line.starts_with("\\ ") {
            i += 1;
            continue;
        }

        if let Some(content) = line.strip_prefix(' ') {
            patch_lines.push(PatchLine::Context(content.to_string()));
        } else if let Some(content) = line.strip_prefix('-') {
            patch_lines.push(PatchLine::Remove(content.to_string()));
        } else if let Some(content) = line.strip_prefix('+') {
            patch_lines.push(PatchLine::Add(content.to_string()));
        } else if line.is_empty() {
            // An empty line in a diff is a context line with empty content.
            patch_lines.push(PatchLine::Context(String::new()));
        } else {
            // Unrecognized line — treat as context (some diffs omit the space prefix
            // for empty context lines).
            patch_lines.push(PatchLine::Context(line.to_string()));
        }

        i += 1;
    }

    Ok((
        PatchHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: patch_lines,
        },
        i,
    ))
}

/// Parse the `@@ -old_start,old_count +new_start,new_count @@` header.
fn parse_hunk_header(header: &str) -> PunchResult<(usize, usize, usize, usize)> {
    // Strip the leading @@ and trailing @@ (and any trailing context text).
    let inner = header
        .strip_prefix("@@ ")
        .and_then(|s| s.find(" @@").map(|pos| &s[..pos]))
        .ok_or_else(|| PunchError::Tool {
            tool: "patch".into(),
            message: format!("malformed hunk header: {}", header),
        })?;

    // inner looks like: "-old_start,old_count +new_start,new_count"
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(PunchError::Tool {
            tool: "patch".into(),
            message: format!("malformed hunk header ranges: {}", header),
        });
    }

    let (old_start, old_count) = parse_range(parts[0], '-')?;
    let (new_start, new_count) = parse_range(parts[1], '+')?;

    Ok((old_start, old_count, new_start, new_count))
}

/// Parse a range like `-3,7` or `+1,4` or `-3` (implicit count = 1) or `+0,0`.
fn parse_range(s: &str, prefix: char) -> PunchResult<(usize, usize)> {
    let s = s.strip_prefix(prefix).ok_or_else(|| PunchError::Tool {
        tool: "patch".into(),
        message: format!("expected '{}' prefix in range '{}'", prefix, s),
    })?;

    if let Some((start_str, count_str)) = s.split_once(',') {
        let start = start_str.parse::<usize>().map_err(|e| PunchError::Tool {
            tool: "patch".into(),
            message: format!("invalid range start '{}': {}", start_str, e),
        })?;
        let count = count_str.parse::<usize>().map_err(|e| PunchError::Tool {
            tool: "patch".into(),
            message: format!("invalid range count '{}': {}", count_str, e),
        })?;
        Ok((start, count))
    } else {
        // No comma — single line, count is implicitly 1.
        let start = s.parse::<usize>().map_err(|e| PunchError::Tool {
            tool: "patch".into(),
            message: format!("invalid range '{}': {}", s, e),
        })?;
        Ok((start, 1))
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Apply a single file patch to the original content — execute the combo.
///
/// Each hunk is applied in order. Context lines are verified against the original
/// to ensure the move connects. Returns the modified content as a string.
pub fn apply_patch(original: &str, patch: &FilePatch) -> PunchResult<String> {
    if patch.is_new_file {
        // New file: original should be empty, just collect added lines.
        let mut result = String::new();
        for hunk in &patch.hunks {
            for line in &hunk.lines {
                if let PatchLine::Add(content) = line {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(content);
                }
            }
        }
        return Ok(result);
    }

    let orig_lines: Vec<&str> = if original.is_empty() {
        Vec::new()
    } else {
        original.lines().collect()
    };

    let mut result_lines: Vec<String> = Vec::new();
    // Current position in the original file (0-based index).
    let mut orig_pos: usize = 0;

    for (hunk_idx, hunk) in patch.hunks.iter().enumerate() {
        // hunk.old_start is 1-based.
        let hunk_start = if hunk.old_start == 0 {
            0
        } else {
            hunk.old_start - 1
        };

        // Copy any lines before this hunk that we haven't consumed yet.
        while orig_pos < hunk_start && orig_pos < orig_lines.len() {
            result_lines.push(orig_lines[orig_pos].to_string());
            orig_pos += 1;
        }

        // Apply the hunk lines.
        for line in &hunk.lines {
            match line {
                PatchLine::Context(content) => {
                    if orig_pos < orig_lines.len() {
                        if orig_lines[orig_pos] != content.as_str() {
                            return Err(PunchError::Tool {
                                tool: "patch".into(),
                                message: format!(
                                    "combo broken at hunk {}: context mismatch at line {} — \
                                     expected {:?}, found {:?}",
                                    hunk_idx + 1,
                                    orig_pos + 1,
                                    content,
                                    orig_lines[orig_pos]
                                ),
                            });
                        }
                        result_lines.push(orig_lines[orig_pos].to_string());
                        orig_pos += 1;
                    } else {
                        return Err(PunchError::Tool {
                            tool: "patch".into(),
                            message: format!(
                                "combo broken at hunk {}: ran out of original lines at context line",
                                hunk_idx + 1,
                            ),
                        });
                    }
                }
                PatchLine::Remove(content) => {
                    if orig_pos < orig_lines.len() {
                        if orig_lines[orig_pos] != content.as_str() {
                            return Err(PunchError::Tool {
                                tool: "patch".into(),
                                message: format!(
                                    "combo broken at hunk {}: remove mismatch at line {} — \
                                     expected {:?}, found {:?}",
                                    hunk_idx + 1,
                                    orig_pos + 1,
                                    content,
                                    orig_lines[orig_pos]
                                ),
                            });
                        }
                        orig_pos += 1;
                    } else {
                        return Err(PunchError::Tool {
                            tool: "patch".into(),
                            message: format!(
                                "combo broken at hunk {}: ran out of original lines for removal",
                                hunk_idx + 1,
                            ),
                        });
                    }
                }
                PatchLine::Add(content) => {
                    result_lines.push(content.clone());
                }
            }
        }
    }

    // Copy remaining lines after the last hunk.
    while orig_pos < orig_lines.len() {
        result_lines.push(orig_lines[orig_pos].to_string());
        orig_pos += 1;
    }

    Ok(result_lines.join("\n"))
}

/// Apply a file patch with fuzzy matching — allow hunks to land at a nearby offset.
///
/// The `fuzz_factor` specifies how many lines of offset to search in each direction
/// when a hunk doesn't land cleanly at its expected position. This is the
/// equivalent of a loose combo that still connects.
pub fn apply_patch_fuzzy(
    original: &str,
    patch: &FilePatch,
    fuzz_factor: usize,
) -> PunchResult<String> {
    if patch.is_new_file {
        return apply_patch(original, patch);
    }

    let orig_lines: Vec<&str> = if original.is_empty() {
        Vec::new()
    } else {
        original.lines().collect()
    };

    let mut result_lines: Vec<String> = Vec::new();
    let mut orig_pos: usize = 0;

    for (hunk_idx, hunk) in patch.hunks.iter().enumerate() {
        let nominal_start = if hunk.old_start == 0 {
            0
        } else {
            hunk.old_start - 1
        };

        // Try to find where the hunk context actually matches.
        let actual_start = find_hunk_match(&orig_lines, hunk, nominal_start, fuzz_factor)
            .ok_or_else(|| PunchError::Tool {
                tool: "patch".into(),
                message: format!(
                    "combo broken at hunk {}: could not find matching context within fuzz factor {}",
                    hunk_idx + 1,
                    fuzz_factor
                ),
            })?;

        // Copy lines between current position and hunk start.
        while orig_pos < actual_start && orig_pos < orig_lines.len() {
            result_lines.push(orig_lines[orig_pos].to_string());
            orig_pos += 1;
        }

        // Apply the hunk.
        for line in &hunk.lines {
            match line {
                PatchLine::Context(_) => {
                    if orig_pos < orig_lines.len() {
                        result_lines.push(orig_lines[orig_pos].to_string());
                        orig_pos += 1;
                    }
                }
                PatchLine::Remove(_) => {
                    if orig_pos < orig_lines.len() {
                        orig_pos += 1;
                    }
                }
                PatchLine::Add(content) => {
                    result_lines.push(content.clone());
                }
            }
        }
    }

    // Copy remaining original lines.
    while orig_pos < orig_lines.len() {
        result_lines.push(orig_lines[orig_pos].to_string());
        orig_pos += 1;
    }

    Ok(result_lines.join("\n"))
}

/// Try to find where a hunk's context/remove lines match in the original,
/// searching around `nominal_start` within `fuzz_factor` lines.
fn find_hunk_match(
    orig_lines: &[&str],
    hunk: &PatchHunk,
    nominal_start: usize,
    fuzz_factor: usize,
) -> Option<usize> {
    // Collect the lines the hunk expects to see in the original (Context + Remove).
    let expected: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|l| match l {
            PatchLine::Context(s) => Some(s.as_str()),
            PatchLine::Remove(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    if expected.is_empty() {
        // Pure addition hunk — matches anywhere.
        return Some(nominal_start.min(orig_lines.len()));
    }

    // Try offset 0 first, then expanding outward.
    for offset in 0..=fuzz_factor {
        // Try at nominal_start + offset
        if let Some(start) = nominal_start.checked_add(offset)
            && matches_at(orig_lines, &expected, start)
        {
            return Some(start);
        }
        // Try at nominal_start - offset (if offset > 0)
        if offset > 0
            && let Some(start) = nominal_start.checked_sub(offset)
            && matches_at(orig_lines, &expected, start)
        {
            return Some(start);
        }
    }

    None
}

/// Check if the expected lines match the original starting at `start`.
fn matches_at(orig_lines: &[&str], expected: &[&str], start: usize) -> bool {
    if start + expected.len() > orig_lines.len() {
        return false;
    }
    expected
        .iter()
        .zip(&orig_lines[start..start + expected.len()])
        .all(|(exp, orig)| exp == orig)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a patch against the original content without applying it.
///
/// Returns a list of conflicts — an empty vec means the combo will land cleanly.
pub fn validate_patch(original: &str, patch: &FilePatch) -> Vec<PatchConflict> {
    let orig_lines: Vec<&str> = if original.is_empty() {
        Vec::new()
    } else {
        original.lines().collect()
    };

    let mut conflicts = Vec::new();

    for (hunk_idx, hunk) in patch.hunks.iter().enumerate() {
        let nominal_start = if hunk.old_start == 0 {
            0
        } else {
            hunk.old_start - 1
        };
        let mut line_pos = nominal_start;

        for line in &hunk.lines {
            match line {
                PatchLine::Context(expected) => {
                    if line_pos >= orig_lines.len() {
                        conflicts.push(PatchConflict {
                            hunk_index: hunk_idx,
                            expected_line: expected.clone(),
                            actual_line: String::new(),
                            line_number: line_pos + 1,
                            conflict_type: ConflictType::LineNotFound,
                        });
                    } else if orig_lines[line_pos] != expected.as_str() {
                        // Check if it can be found at a nearby offset.
                        let found_offset = find_line_nearby(&orig_lines, expected, line_pos, 10);
                        let conflict_type = if let Some(actual_pos) = found_offset {
                            ConflictType::OffsetApplied(actual_pos as i32 - line_pos as i32)
                        } else {
                            ConflictType::ContextMismatch
                        };
                        conflicts.push(PatchConflict {
                            hunk_index: hunk_idx,
                            expected_line: expected.clone(),
                            actual_line: orig_lines[line_pos].to_string(),
                            line_number: line_pos + 1,
                            conflict_type,
                        });
                    }
                    line_pos += 1;
                }
                PatchLine::Remove(expected) => {
                    if line_pos >= orig_lines.len() {
                        conflicts.push(PatchConflict {
                            hunk_index: hunk_idx,
                            expected_line: expected.clone(),
                            actual_line: String::new(),
                            line_number: line_pos + 1,
                            conflict_type: ConflictType::LineNotFound,
                        });
                    } else if orig_lines[line_pos] != expected.as_str() {
                        conflicts.push(PatchConflict {
                            hunk_index: hunk_idx,
                            expected_line: expected.clone(),
                            actual_line: orig_lines[line_pos].to_string(),
                            line_number: line_pos + 1,
                            conflict_type: ConflictType::ContextMismatch,
                        });
                    }
                    line_pos += 1;
                }
                PatchLine::Add(_) => {
                    // Additions don't consume original lines — no conflict possible.
                }
            }
        }
    }

    conflicts
}

/// Search for a line near `pos` within `radius` lines.
fn find_line_nearby(orig_lines: &[&str], target: &str, pos: usize, radius: usize) -> Option<usize> {
    for offset in 1..=radius {
        if let Some(p) = pos.checked_add(offset)
            && p < orig_lines.len()
            && orig_lines[p] == target
        {
            return Some(p);
        }
        if let Some(p) = pos.checked_sub(offset)
            && orig_lines[p] == target
        {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Diff generation — simplified Myers-style algorithm
// ---------------------------------------------------------------------------

/// Generate a unified diff from two strings — choreograph the combo notation.
///
/// Uses a simplified line-by-line diff algorithm based on longest common
/// subsequence (LCS) to produce standard unified diff output with context lines.
pub fn generate_unified_diff(
    old_content: &str,
    new_content: &str,
    old_path: &str,
    new_path: &str,
) -> String {
    let old_lines: Vec<&str> = if old_content.is_empty() {
        Vec::new()
    } else {
        old_content.lines().collect()
    };
    let new_lines: Vec<&str> = if new_content.is_empty() {
        Vec::new()
    } else {
        new_content.lines().collect()
    };

    let edits = compute_edit_script(&old_lines, &new_lines);
    let hunks = edits_to_hunks(&old_lines, &new_lines, &edits, 3);

    if hunks.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    output.push_str(&format!("--- a/{}\n", old_path));
    output.push_str(&format!("+++ b/{}\n", new_path));

    for hunk in &hunks {
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));
        for line in &hunk.lines {
            match line {
                PatchLine::Context(s) => {
                    output.push(' ');
                    output.push_str(s);
                    output.push('\n');
                }
                PatchLine::Remove(s) => {
                    output.push('-');
                    output.push_str(s);
                    output.push('\n');
                }
                PatchLine::Add(s) => {
                    output.push('+');
                    output.push_str(s);
                    output.push('\n');
                }
            }
        }
    }

    output
}

/// Edit operation for the diff algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditOp {
    /// Line is the same in both files.
    Equal,
    /// Line was removed from the old file.
    Delete,
    /// Line was inserted in the new file.
    Insert,
}

/// Compute a line-level edit script using LCS.
fn compute_edit_script<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(EditOp, usize, usize)> {
    let m = old.len();
    let n = new.len();

    // Build LCS table.
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to produce the edit script.
    let mut edits = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            edits.push((EditOp::Equal, i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            edits.push((EditOp::Insert, i, j - 1));
            j -= 1;
        } else if i > 0 {
            edits.push((EditOp::Delete, i - 1, j));
            i -= 1;
        }
    }

    edits.reverse();
    edits
}

/// Group edit operations into hunks with context lines.
fn edits_to_hunks(
    old_lines: &[&str],
    new_lines: &[&str],
    edits: &[(EditOp, usize, usize)],
    context: usize,
) -> Vec<PatchHunk> {
    if edits.is_empty() {
        return Vec::new();
    }

    // Find ranges of non-equal edits, then expand with context.
    let mut change_ranges: Vec<(usize, usize)> = Vec::new(); // (start_idx, end_idx) in edits
    let mut i = 0;
    while i < edits.len() {
        if edits[i].0 != EditOp::Equal {
            let start = i;
            while i < edits.len() && edits[i].0 != EditOp::Equal {
                i += 1;
            }
            change_ranges.push((start, i));
        } else {
            i += 1;
        }
    }

    if change_ranges.is_empty() {
        return Vec::new();
    }

    // Merge nearby change ranges and build hunks with context.
    let mut hunks = Vec::new();
    let mut range_idx = 0;

    while range_idx < change_ranges.len() {
        let (first_start, _) = change_ranges[range_idx];

        // Find how many ranges to merge (those within 2*context of each other).
        let mut last_end = change_ranges[range_idx].1;
        let mut merge_end = range_idx;
        while merge_end + 1 < change_ranges.len() {
            let next_start = change_ranges[merge_end + 1].0;
            // If the gap between ranges is <= 2*context, merge them.
            if next_start - last_end <= 2 * context {
                merge_end += 1;
                last_end = change_ranges[merge_end].1;
            } else {
                break;
            }
        }

        // Build a hunk covering first_start..last_end with context.
        let hunk_edit_start = first_start.saturating_sub(context).max(0);
        let hunk_edit_end = last_end
            .min(edits.len())
            .saturating_add(context)
            .min(edits.len());

        let mut hunk_lines = Vec::new();
        let mut old_line_start = usize::MAX;
        let mut old_count = 0usize;
        let mut new_line_start = usize::MAX;
        let mut new_count = 0usize;

        for edit_idx in hunk_edit_start..hunk_edit_end {
            if edit_idx >= edits.len() {
                break;
            }
            let (op, old_idx, new_idx) = edits[edit_idx];

            match op {
                EditOp::Equal => {
                    if old_idx < old_lines.len() {
                        hunk_lines.push(PatchLine::Context(old_lines[old_idx].to_string()));
                        if old_line_start == usize::MAX {
                            old_line_start = old_idx;
                            new_line_start = new_idx;
                        }
                        old_count += 1;
                        new_count += 1;
                    }
                }
                EditOp::Delete => {
                    if old_idx < old_lines.len() {
                        hunk_lines.push(PatchLine::Remove(old_lines[old_idx].to_string()));
                        if old_line_start == usize::MAX {
                            old_line_start = old_idx;
                            new_line_start = new_idx;
                        }
                        old_count += 1;
                    }
                }
                EditOp::Insert => {
                    if new_idx < new_lines.len() {
                        hunk_lines.push(PatchLine::Add(new_lines[new_idx].to_string()));
                        if old_line_start == usize::MAX {
                            old_line_start = old_idx;
                            new_line_start = new_idx;
                        }
                        new_count += 1;
                    }
                }
            }
        }

        if !hunk_lines.is_empty() {
            hunks.push(PatchHunk {
                old_start: if old_line_start == usize::MAX {
                    0
                } else {
                    old_line_start + 1
                },
                old_count,
                new_start: if new_line_start == usize::MAX {
                    0
                } else {
                    new_line_start + 1
                },
                new_count,
                lines: hunk_lines,
            });
        }

        range_idx = merge_end + 1;
    }

    hunks
}

// ---------------------------------------------------------------------------
// Rollback — reverse the combo
// ---------------------------------------------------------------------------

/// Create a reverse patch — swap additions and removals, old and new paths.
///
/// Applying the reversed patch to the patched content yields the original.
/// When a combo goes wrong, this is how you rewind the tape.
pub fn reverse_patch(patch: &FilePatch) -> FilePatch {
    let reversed_hunks: Vec<PatchHunk> = patch
        .hunks
        .iter()
        .map(|hunk| {
            let reversed_lines: Vec<PatchLine> = hunk
                .lines
                .iter()
                .map(|line| match line {
                    PatchLine::Context(s) => PatchLine::Context(s.clone()),
                    PatchLine::Remove(s) => PatchLine::Add(s.clone()),
                    PatchLine::Add(s) => PatchLine::Remove(s.clone()),
                })
                .collect();

            PatchHunk {
                old_start: hunk.new_start,
                old_count: hunk.new_count,
                new_start: hunk.old_start,
                new_count: hunk.old_count,
                lines: reversed_lines,
            }
        })
        .collect();

    FilePatch {
        old_path: patch.new_path.clone(),
        new_path: patch.old_path.clone(),
        hunks: reversed_hunks,
        is_new_file: patch.is_deleted,
        is_deleted: patch.is_new_file,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_unified_diff() {
        let diff = "\
--- a/hello.rs
+++ b/hello.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello, world\");
+    println!(\"goodbye\");
 }
";
        let ps = parse_unified_diff(diff).expect("should parse");
        assert_eq!(ps.patches.len(), 1);
        let fp = &ps.patches[0];
        assert_eq!(fp.old_path, "hello.rs");
        assert_eq!(fp.new_path, "hello.rs");
        assert!(!fp.is_new_file);
        assert!(!fp.is_deleted);
        assert_eq!(fp.hunks.len(), 1);
        let h = &fp.hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.old_count, 3);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.new_count, 4);
        assert_eq!(h.lines.len(), 5);
    }

    #[test]
    fn test_parse_multi_hunk_diff() {
        let diff = "\
--- a/lib.rs
+++ b/lib.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old_a();
+    new_a();
 }
@@ -10,3 +10,3 @@
 fn b() {
-    old_b();
+    new_b();
 }
";
        let ps = parse_unified_diff(diff).expect("should parse");
        assert_eq!(ps.patches.len(), 1);
        assert_eq!(ps.patches[0].hunks.len(), 2);
        assert_eq!(ps.patches[0].hunks[0].old_start, 1);
        assert_eq!(ps.patches[0].hunks[1].old_start, 10);
    }

    #[test]
    fn test_parse_new_file_diff() {
        let diff = "\
--- /dev/null
+++ b/new_file.rs
@@ -0,0 +1,3 @@
+fn new_func() {
+    // brand new
+}
";
        let ps = parse_unified_diff(diff).expect("should parse");
        assert_eq!(ps.patches.len(), 1);
        assert!(ps.patches[0].is_new_file);
        assert!(!ps.patches[0].is_deleted);
        assert_eq!(ps.patches[0].new_path, "new_file.rs");
    }

    #[test]
    fn test_parse_deleted_file_diff() {
        let diff = "\
--- a/dead.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn old() {
-    // going away
-}
";
        let ps = parse_unified_diff(diff).expect("should parse");
        assert_eq!(ps.patches.len(), 1);
        assert!(!ps.patches[0].is_new_file);
        assert!(ps.patches[0].is_deleted);
        assert_eq!(ps.patches[0].old_path, "dead.rs");
    }

    #[test]
    fn test_apply_simple_addition() {
        let original = "line1\nline2\nline3";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 2,
                old_count: 1,
                new_start: 2,
                new_count: 2,
                lines: vec![
                    PatchLine::Context("line2".into()),
                    PatchLine::Add("inserted".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let result = apply_patch(original, &patch).expect("should apply");
        assert_eq!(result, "line1\nline2\ninserted\nline3");
    }

    #[test]
    fn test_apply_simple_deletion() {
        let original = "line1\nline2\nline3";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 2,
                lines: vec![
                    PatchLine::Context("line1".into()),
                    PatchLine::Remove("line2".into()),
                    PatchLine::Context("line3".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let result = apply_patch(original, &patch).expect("should apply");
        assert_eq!(result, "line1\nline3");
    }

    #[test]
    fn test_apply_modification() {
        let original = "fn main() {\n    println!(\"old\");\n}";
        let patch = FilePatch {
            old_path: "f.rs".into(),
            new_path: "f.rs".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 3,
                lines: vec![
                    PatchLine::Context("fn main() {".into()),
                    PatchLine::Remove("    println!(\"old\");".into()),
                    PatchLine::Add("    println!(\"new\");".into()),
                    PatchLine::Context("}".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let result = apply_patch(original, &patch).expect("should apply");
        assert_eq!(result, "fn main() {\n    println!(\"new\");\n}");
    }

    #[test]
    fn test_apply_multi_hunk() {
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![
                PatchHunk {
                    old_start: 2,
                    old_count: 1,
                    new_start: 2,
                    new_count: 1,
                    lines: vec![PatchLine::Remove("b".into()), PatchLine::Add("B".into())],
                },
                PatchHunk {
                    old_start: 8,
                    old_count: 1,
                    new_start: 8,
                    new_count: 1,
                    lines: vec![PatchLine::Remove("h".into()), PatchLine::Add("H".into())],
                },
            ],
            is_new_file: false,
            is_deleted: false,
        };
        let result = apply_patch(original, &patch).expect("should apply");
        assert_eq!(result, "a\nB\nc\nd\ne\nf\ng\nH\ni\nj");
    }

    #[test]
    fn test_apply_new_file_patch() {
        let patch = FilePatch {
            old_path: "/dev/null".into(),
            new_path: "new.rs".into(),
            hunks: vec![PatchHunk {
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: 2,
                lines: vec![
                    PatchLine::Add("// new file".into()),
                    PatchLine::Add("fn hello() {}".into()),
                ],
            }],
            is_new_file: true,
            is_deleted: false,
        };
        let result = apply_patch("", &patch).expect("should apply");
        assert_eq!(result, "// new file\nfn hello() {}");
    }

    #[test]
    fn test_fuzzy_matching_with_offset() {
        // The hunk says it starts at line 3, but the content has shifted by 2 lines.
        let original = "extra1\nextra2\na\nb\nc";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 2,
                lines: vec![
                    PatchLine::Context("a".into()),
                    PatchLine::Remove("b".into()),
                    PatchLine::Add("B".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        // Strict apply should fail.
        assert!(apply_patch(original, &patch).is_err());
        // Fuzzy with fuzz_factor=3 should succeed.
        let result = apply_patch_fuzzy(original, &patch, 3).expect("should apply fuzzy");
        assert_eq!(result, "extra1\nextra2\na\nB\nc");
    }

    #[test]
    fn test_validate_clean_patch() {
        let original = "line1\nline2\nline3";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 3,
                lines: vec![
                    PatchLine::Context("line1".into()),
                    PatchLine::Remove("line2".into()),
                    PatchLine::Add("LINE2".into()),
                    PatchLine::Context("line3".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let conflicts = validate_patch(original, &patch);
        assert!(
            conflicts.is_empty(),
            "expected no conflicts, got {:?}",
            conflicts
        );
    }

    #[test]
    fn test_validate_conflicting_patch() {
        let original = "line1\nDIFFERENT\nline3";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 3,
                lines: vec![
                    PatchLine::Context("line1".into()),
                    PatchLine::Remove("line2".into()),
                    PatchLine::Add("LINE2".into()),
                    PatchLine::Context("line3".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let conflicts = validate_patch(original, &patch);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].expected_line, "line2");
        assert_eq!(conflicts[0].actual_line, "DIFFERENT");
        assert_eq!(conflicts[0].conflict_type, ConflictType::ContextMismatch);
    }

    #[test]
    fn test_generate_diff_from_two_strings() {
        let old = "line1\nline2\nline3";
        let new = "line1\nmodified\nline3";
        let diff = generate_unified_diff(old, new, "file.txt", "file.txt");
        assert!(diff.contains("--- a/file.txt"));
        assert!(diff.contains("+++ b/file.txt"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }

    #[test]
    fn test_generated_diff_can_be_parsed_back() {
        let old = "alpha\nbeta\ngamma\ndelta";
        let new = "alpha\nBETA\ngamma\ndelta";
        let diff = generate_unified_diff(old, new, "test.txt", "test.txt");
        let parsed = parse_unified_diff(&diff).expect("should parse generated diff");
        assert_eq!(parsed.patches.len(), 1);
        assert!(!parsed.patches[0].hunks.is_empty());
    }

    #[test]
    fn test_round_trip_generate_parse_apply() {
        let old = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}";
        let new = "fn main() {\n    println!(\"world\");\n    let x = 1;\n    let y = 2;\n}";
        let diff = generate_unified_diff(old, new, "main.rs", "main.rs");
        let parsed = parse_unified_diff(&diff).expect("should parse");
        assert_eq!(parsed.patches.len(), 1);
        let result = apply_patch(old, &parsed.patches[0]).expect("should apply");
        assert_eq!(result, new);
    }

    #[test]
    fn test_reverse_patch_roundtrip() {
        let original = "line1\nline2\nline3";
        let patch = FilePatch {
            old_path: "f.txt".into(),
            new_path: "f.txt".into(),
            hunks: vec![PatchHunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 3,
                lines: vec![
                    PatchLine::Context("line1".into()),
                    PatchLine::Remove("line2".into()),
                    PatchLine::Add("CHANGED".into()),
                    PatchLine::Context("line3".into()),
                ],
            }],
            is_new_file: false,
            is_deleted: false,
        };
        let patched = apply_patch(original, &patch).expect("should apply");
        assert_eq!(patched, "line1\nCHANGED\nline3");

        let reversed = reverse_patch(&patch);
        let restored = apply_patch(&patched, &reversed).expect("should apply reverse");
        assert_eq!(restored, original);
    }

    #[test]
    fn test_empty_diff() {
        let ps = parse_unified_diff("").expect("should parse empty");
        assert!(ps.patches.is_empty());
    }

    #[test]
    fn test_hunk_header_various_formats() {
        // Standard format with comma.
        let (s, c, ns, nc) = parse_hunk_header("@@ -1,3 +1,4 @@").expect("should parse");
        assert_eq!((s, c, ns, nc), (1, 3, 1, 4));

        // No comma (single line).
        let (s, c, ns, nc) = parse_hunk_header("@@ -1 +1 @@").expect("should parse");
        assert_eq!((s, c, ns, nc), (1, 1, 1, 1));

        // With trailing context text after @@.
        let (s, c, ns, nc) =
            parse_hunk_header("@@ -10,5 +12,7 @@ fn some_function()").expect("should parse");
        assert_eq!((s, c, ns, nc), (10, 5, 12, 7));

        // Zero-line ranges (new file / deleted file).
        let (s, c, ns, nc) = parse_hunk_header("@@ -0,0 +1,3 @@").expect("should parse");
        assert_eq!((s, c, ns, nc), (0, 0, 1, 3));
    }

    #[test]
    fn test_parse_diff_with_git_prefix() {
        let diff = "\
diff --git a/foo.rs b/foo.rs
index abc123..def456 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,2 @@
 line1
-old
+new
";
        let ps = parse_unified_diff(diff).expect("should parse with git prefix");
        assert_eq!(ps.patches.len(), 1);
        assert_eq!(ps.patches[0].old_path, "foo.rs");
        assert_eq!(ps.patches[0].hunks.len(), 1);
    }

    #[test]
    fn test_generate_empty_diff_for_identical_content() {
        let content = "same\ncontent\nhere";
        let diff = generate_unified_diff(content, content, "f.txt", "f.txt");
        assert!(
            diff.is_empty(),
            "identical content should produce empty diff"
        );
    }
}
