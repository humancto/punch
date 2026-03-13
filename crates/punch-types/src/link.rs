//! # Link Understanding — scouting enemy territory by extracting intel from URLs.
//!
//! This module provides types and traits for fetching and extracting structured
//! content from URLs, turning raw links into actionable battlefield intelligence.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::PunchResult;

/// Classification of the content behind a link — what kind of territory we're scouting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkContentType {
    /// A written article or blog post.
    Article,
    /// Technical documentation.
    Documentation,
    /// Code repository (GitHub, GitLab, etc.).
    Repository,
    /// Social media post or thread.
    SocialMedia,
    /// Video content page.
    Video,
    /// Image content page.
    Image,
    /// Unclassified content.
    Other,
}

/// Metadata extracted from a link — the dossier on the target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkMetadata {
    /// Author of the content.
    pub author: Option<String>,
    /// When the content was published.
    pub published_at: Option<DateTime<Utc>>,
    /// Approximate word count of the main content.
    pub word_count: usize,
    /// Detected language of the content.
    pub language: Option<String>,
    /// Short description or summary.
    pub description: Option<String>,
}

impl LinkMetadata {
    /// Create empty metadata with zero word count.
    pub fn empty() -> Self {
        Self {
            author: None,
            published_at: None,
            word_count: 0,
            language: None,
            description: None,
        }
    }
}

/// Extracted content from a URL — the full intelligence report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkContent {
    /// The original URL that was extracted.
    pub url: String,
    /// Title of the page/content.
    pub title: Option<String>,
    /// The extracted main text content.
    pub content: String,
    /// Classification of the content type.
    pub content_type: LinkContentType,
    /// Metadata about the content.
    pub metadata: LinkMetadata,
}

impl LinkContent {
    /// Create a new link content result.
    pub fn new(
        url: impl Into<String>,
        content: impl Into<String>,
        content_type: LinkContentType,
    ) -> Self {
        let content = content.into();
        let word_count = content.split_whitespace().count();
        Self {
            url: url.into(),
            title: None,
            content,
            content_type,
            metadata: LinkMetadata {
                author: None,
                published_at: None,
                word_count,
                language: None,
                description: None,
            },
        }
    }

    /// Set the title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: LinkMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Trait for link extraction backends — the scout unit that infiltrates URLs.
#[async_trait]
pub trait LinkExtractor: Send + Sync {
    /// Extract content from the given URL.
    async fn extract(&self, url: &str) -> PunchResult<LinkContent>;

    /// Check if this extractor supports the given URL.
    fn supports_url(&self, url: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_construction() {
        let content = LinkContent::new(
            "https://example.com/article",
            "This is a test article about fighting techniques.",
            LinkContentType::Article,
        )
        .with_title("Fighting Techniques");

        assert_eq!(content.url, "https://example.com/article");
        assert_eq!(content.title, Some("Fighting Techniques".to_string()));
        assert_eq!(content.content_type, LinkContentType::Article);
        assert!(!content.content.is_empty());
    }

    #[test]
    fn test_content_type_classification() {
        let types = vec![
            LinkContentType::Article,
            LinkContentType::Documentation,
            LinkContentType::Repository,
            LinkContentType::SocialMedia,
            LinkContentType::Video,
            LinkContentType::Image,
            LinkContentType::Other,
        ];

        for ct in &types {
            let json = serde_json::to_string(ct).expect("serialize content type");
            let deser: LinkContentType =
                serde_json::from_str(&json).expect("deserialize content type");
            assert_eq!(&deser, ct);
        }

        assert_eq!(
            serde_json::to_string(&LinkContentType::SocialMedia).expect("social media"),
            "\"social_media\""
        );
    }

    #[test]
    fn test_metadata() {
        let metadata = LinkMetadata {
            author: Some("The Champion".to_string()),
            published_at: Some(Utc::now()),
            word_count: 1500,
            language: Some("en".to_string()),
            description: Some("A guide to winning".to_string()),
        };

        let json = serde_json::to_string(&metadata).expect("serialize metadata");
        let deser: LinkMetadata = serde_json::from_str(&json).expect("deserialize metadata");

        assert_eq!(deser.author, Some("The Champion".to_string()));
        assert_eq!(deser.word_count, 1500);
        assert_eq!(deser.language, Some("en".to_string()));
    }

    #[test]
    fn test_url_support_check() {
        // Test that supports_url can be implemented with simple pattern matching.
        let github_url = "https://github.com/humancto/punch";
        let docs_url = "https://docs.rs/serde/latest";
        let random_url = "https://example.com/page";

        // Simple URL classification logic for testing.
        fn classify_url(url: &str) -> LinkContentType {
            if url.contains("github.com") {
                LinkContentType::Repository
            } else if url.contains("docs.rs") || url.contains("docs.") {
                LinkContentType::Documentation
            } else {
                LinkContentType::Other
            }
        }

        assert_eq!(classify_url(github_url), LinkContentType::Repository);
        assert_eq!(classify_url(docs_url), LinkContentType::Documentation);
        assert_eq!(classify_url(random_url), LinkContentType::Other);
    }

    #[test]
    fn test_word_count() {
        let content = LinkContent::new(
            "https://example.com",
            "one two three four five six seven eight nine ten",
            LinkContentType::Article,
        );

        assert_eq!(content.metadata.word_count, 10);

        let empty_content = LinkContent::new("https://example.com", "", LinkContentType::Other);
        assert_eq!(empty_content.metadata.word_count, 0);
    }
}
