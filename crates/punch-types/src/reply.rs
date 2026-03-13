//! # Reply Directives — the battle stance for formatting and delivering responses.
//!
//! This module controls how fighters format and deliver their responses to the arena,
//! enforcing structure, tone, and presentation rules on outgoing strikes.

use serde::{Deserialize, Serialize};

/// The format a reply should take — the shape of the strike.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyFormat {
    /// Markdown-formatted text.
    Markdown,
    /// Plain text with no formatting.
    PlainText,
    /// HTML-formatted text.
    Html,
    /// Raw JSON output.
    Json,
    /// Code block in the specified language.
    Code(String),
    /// Tabular format.
    Table,
    /// Bulleted list format.
    Bullet,
}

/// The tone of a reply — the fighting spirit behind the words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyTone {
    /// Formal, business-appropriate tone.
    Professional,
    /// Relaxed, conversational tone.
    Casual,
    /// Precise, jargon-rich technical tone.
    Technical,
    /// Warm, approachable tone.
    Friendly,
    /// Terse, to-the-point tone.
    Concise,
    /// Thorough, explanatory tone.
    Detailed,
}

/// A directive controlling how a response is formatted and delivered — the battle plan for output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyDirective {
    /// The format to use for the response.
    pub format: ReplyFormat,
    /// Maximum length in characters (truncate if exceeded).
    pub max_length: Option<usize>,
    /// Desired tone of the response.
    pub tone: Option<ReplyTone>,
    /// Language for the response (e.g., "en", "ja").
    pub language: Option<String>,
    /// Target audience description.
    pub audience: Option<String>,
    /// Whether to include source citations.
    pub include_sources: bool,
    /// JSON schema for structured output validation.
    pub structured_output: Option<serde_json::Value>,
}

impl ReplyDirective {
    /// Create a new reply directive with the given format and sensible defaults.
    pub fn new(format: ReplyFormat) -> Self {
        Self {
            format,
            max_length: None,
            tone: None,
            language: None,
            audience: None,
            include_sources: false,
            structured_output: None,
        }
    }

    /// Set the maximum length.
    pub fn with_max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    /// Set the tone.
    pub fn with_tone(mut self, tone: ReplyTone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// Set the language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set the audience.
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Enable source inclusion.
    pub fn with_sources(mut self) -> Self {
        self.include_sources = true;
        self
    }

    /// Set a JSON schema for structured output.
    pub fn with_structured_output(mut self, schema: serde_json::Value) -> Self {
        self.structured_output = Some(schema);
        self
    }
}

/// Apply a reply directive to content — execute the formatting battle plan.
///
/// This function transforms raw content according to the directive's rules:
/// - Truncates to `max_length` if set
/// - Wraps in code blocks if `Code` format
/// - Strips markdown formatting if `PlainText` format
pub fn apply_directive(content: &str, directive: &ReplyDirective) -> String {
    let mut result = match &directive.format {
        ReplyFormat::Code(language) => {
            format!("```{language}\n{content}\n```")
        }
        ReplyFormat::PlainText => strip_markdown(content),
        _ => content.to_string(),
    };

    if let Some(max_len) = directive.max_length
        && result.len() > max_len
    {
        result.truncate(max_len);
        // Try to truncate at a word boundary.
        if let Some(last_space) = result.rfind(' ') {
            result.truncate(last_space);
        }
        result.push_str("...");
    }

    result
}

/// Strip basic markdown formatting from text — disarm the formatting weapons.
fn strip_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    for line in text.lines() {
        let stripped = line.trim();

        // Remove heading markers.
        if stripped.starts_with('#') {
            let without_hashes = stripped.trim_start_matches('#').trim_start();
            result.push_str(without_hashes);
        }
        // Remove bullet markers.
        else if stripped.starts_with("- ") || stripped.starts_with("* ") {
            result.push_str(&stripped[2..]);
        }
        // Remove numbered list markers.
        else if stripped.chars().take_while(|c| c.is_ascii_digit()).count() > 0
            && stripped.contains(". ")
        {
            if let Some(pos) = stripped.find(". ") {
                let prefix = &stripped[..pos];
                if prefix.chars().all(|c| c.is_ascii_digit()) {
                    result.push_str(&stripped[pos + 2..]);
                } else {
                    result.push_str(stripped);
                }
            } else {
                result.push_str(stripped);
            }
        } else {
            result.push_str(stripped);
        }
        result.push('\n');
    }

    // Remove bold/italic/code markers.
    let result = result
        .replace("**", "")
        .replace("__", "")
        .replace(['*', '_', '`'], "");

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directive_creation() {
        let directive = ReplyDirective::new(ReplyFormat::Markdown)
            .with_tone(ReplyTone::Professional)
            .with_max_length(500)
            .with_language("en")
            .with_audience("developers")
            .with_sources();

        assert_eq!(directive.format, ReplyFormat::Markdown);
        assert_eq!(directive.tone, Some(ReplyTone::Professional));
        assert_eq!(directive.max_length, Some(500));
        assert_eq!(directive.language, Some("en".to_string()));
        assert_eq!(directive.audience, Some("developers".to_string()));
        assert!(directive.include_sources);
    }

    #[test]
    fn test_apply_truncation() {
        let directive = ReplyDirective::new(ReplyFormat::Markdown).with_max_length(20);

        let content = "This is a long piece of content that should be truncated";
        let result = apply_directive(content, &directive);

        assert!(result.ends_with("..."));
        // The truncated content (before "...") should be at most 20 chars.
        assert!(result.len() <= 20 + 3); // max_length + "..."
    }

    #[test]
    fn test_apply_code_format() {
        let directive = ReplyDirective::new(ReplyFormat::Code("rust".to_string()));

        let content = "fn main() {}";
        let result = apply_directive(content, &directive);

        assert!(result.starts_with("```rust\n"));
        assert!(result.ends_with("\n```"));
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn test_apply_plain_text() {
        let directive = ReplyDirective::new(ReplyFormat::PlainText);

        let content = "# Heading\n\n**Bold text** and *italic text*\n\n- Item one\n- Item two";
        let result = apply_directive(content, &directive);

        assert!(!result.contains('#'));
        assert!(!result.contains("**"));
        assert!(!result.contains('*'));
        assert!(result.contains("Heading"));
        assert!(result.contains("Bold text"));
    }

    #[test]
    fn test_format_serialization() {
        let formats = vec![
            ReplyFormat::Markdown,
            ReplyFormat::PlainText,
            ReplyFormat::Html,
            ReplyFormat::Json,
            ReplyFormat::Code("python".to_string()),
            ReplyFormat::Table,
            ReplyFormat::Bullet,
        ];

        for fmt in &formats {
            let json = serde_json::to_string(fmt).expect("serialize format");
            let deser: ReplyFormat = serde_json::from_str(&json).expect("deserialize format");
            assert_eq!(&deser, fmt);
        }

        let tones = vec![
            ReplyTone::Professional,
            ReplyTone::Casual,
            ReplyTone::Technical,
            ReplyTone::Friendly,
            ReplyTone::Concise,
            ReplyTone::Detailed,
        ];

        for tone in &tones {
            let json = serde_json::to_string(tone).expect("serialize tone");
            let deser: ReplyTone = serde_json::from_str(&json).expect("deserialize tone");
            assert_eq!(&deser, tone);
        }
    }
}
