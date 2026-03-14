//! GitHub adapter for issue and pull request comments.
//!
//! Sends comments via the GitHub REST API and parses incoming webhook
//! payloads for issue_comment and pull_request_review_comment events.
//! Includes HMAC-SHA256 webhook signature verification.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const GITHUB_API_BASE: &str = "https://api.github.com";

type HmacSha256 = Hmac<Sha256>;

/// GitHub adapter for issue/PR comment interactions.
///
/// Receives: GitHub webhook payloads (issue_comment, pull_request_review_comment).
/// Sends: comments via the GitHub REST API.
pub struct GitHubAdapter {
    /// Personal access token or GitHub App installation token.
    token: String,
    /// Repository owner (user or organization).
    owner: String,
    /// Repository name.
    repo: String,
    /// Webhook secret for verifying payload signatures.
    webhook_secret: String,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl GitHubAdapter {
    /// Create a new GitHub adapter.
    ///
    /// `token`: GitHub personal access token or app installation token.
    /// `owner`: Repository owner (user or organization).
    /// `repo`: Repository name.
    /// `webhook_secret`: Secret for verifying webhook payload signatures.
    pub fn new(token: String, owner: String, repo: String, webhook_secret: String) -> Self {
        Self {
            token,
            owner,
            repo,
            webhook_secret,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Verify a GitHub webhook signature (HMAC-SHA256).
    ///
    /// `signature`: The value of the `X-Hub-Signature-256` header (e.g. "sha256=abc...").
    /// `body`: The raw request body bytes.
    pub fn verify_webhook_signature(&self, signature: &str, body: &[u8]) -> bool {
        let expected_prefix = "sha256=";
        let hex_signature = match signature.strip_prefix(expected_prefix) {
            Some(hex) => hex,
            None => return false,
        };

        let mut mac = match HmacSha256::new_from_slice(self.webhook_secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(body);

        let expected = mac.finalize().into_bytes();
        let expected_hex = hex_encode(&expected);

        // Constant-time comparison
        constant_time_eq(expected_hex.as_bytes(), hex_signature.as_bytes())
    }

    /// Parse a GitHub webhook payload into an `IncomingMessage`.
    ///
    /// Supports `issue_comment` and `pull_request_review_comment` events.
    ///
    /// Expected JSON format (issue_comment):
    /// ```json
    /// {
    ///   "action": "created",
    ///   "issue": { "number": 42, "title": "Bug report" },
    ///   "comment": {
    ///     "id": 12345,
    ///     "user": { "login": "alice", "id": 67890 },
    ///     "body": "@bot help me",
    ///     "created_at": "2024-01-15T12:00:00Z"
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Option<IncomingMessage> {
        let action = payload.get("action")?.as_str()?;
        if action != "created" {
            return None;
        }

        match event_type {
            "issue_comment" => self.parse_issue_comment(payload),
            "pull_request_review_comment" => self.parse_pr_review_comment(payload),
            _ => None,
        }
    }

    fn parse_issue_comment(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let issue = payload.get("issue")?;
        let comment = payload.get("comment")?;

        let issue_number = issue.get("number")?.as_u64()?;
        let comment_id = comment.get("id")?.as_u64()?;
        let user = comment.get("user")?;
        let login = user.get("login")?.as_str()?;
        let user_id = user.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let body = comment.get("body")?.as_str()?;
        if body.is_empty() {
            return None;
        }

        let created_at = comment
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        metadata.insert(
            "event_type".to_string(),
            serde_json::Value::String("issue_comment".to_string()),
        );
        metadata.insert("issue_number".to_string(), serde_json::json!(issue_number));
        if let Some(title) = issue.get("title").and_then(|v| v.as_str()) {
            metadata.insert(
                "issue_title".to_string(),
                serde_json::Value::String(title.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: issue_number.to_string(),
            user_id: user_id.to_string(),
            display_name: login.to_string(),
            text: body.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::GitHub,
            platform_message_id: comment_id.to_string(),
            is_group: true,
            metadata,
        })
    }

    fn parse_pr_review_comment(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let pull_request = payload.get("pull_request")?;
        let comment = payload.get("comment")?;

        let pr_number = pull_request.get("number")?.as_u64()?;
        let comment_id = comment.get("id")?.as_u64()?;
        let user = comment.get("user")?;
        let login = user.get("login")?.as_str()?;
        let user_id = user.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let body = comment.get("body")?.as_str()?;
        if body.is_empty() {
            return None;
        }

        let created_at = comment
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        metadata.insert(
            "event_type".to_string(),
            serde_json::Value::String("pull_request_review_comment".to_string()),
        );
        metadata.insert("pr_number".to_string(), serde_json::json!(pr_number));
        if let Some(path) = comment.get("path").and_then(|v| v.as_str()) {
            metadata.insert(
                "file_path".to_string(),
                serde_json::Value::String(path.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: pr_number.to_string(),
            user_id: user_id.to_string(),
            display_name: login.to_string(),
            text: body.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::GitHub,
            platform_message_id: comment_id.to_string(),
            is_group: true,
            metadata,
        })
    }

    /// Post a comment on an issue or pull request.
    async fn api_post_comment(&self, issue_number: &str, text: &str) -> PunchResult<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments",
            GITHUB_API_BASE, self.owner, self.repo, issue_number
        );

        let body = serde_json::json!({
            "body": text,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "punch-agent-os")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "github".to_string(),
                message: format!("failed to post comment: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("GitHub post comment failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[async_trait]
impl ChannelAdapter for GitHubAdapter {
    fn name(&self) -> &str {
        "github"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::GitHub
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(
            owner = %self.owner,
            repo = %self.repo,
            "GitHub adapter started (webhook mode)"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("GitHub adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_post_comment(channel_id, message).await
    }

    fn status(&self) -> ChannelStatus {
        ChannelStatus {
            connected: self.running.load(Ordering::Relaxed),
            started_at: self.started_at.try_read().ok().and_then(|g| *g),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            last_error: None,
        }
    }

    async fn validate_credentials(&self) -> PunchResult<()> {
        let resp = self
            .client
            .get(format!("{}/user", GITHUB_API_BASE))
            .header("Authorization", format!("token {}", self.token))
            .header("User-Agent", "punch-agent-os")
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "github".to_string(),
                message: format!("credential validation failed: {}", e),
            })?;
        if !resp.status().is_success() {
            return Err(PunchError::Channel {
                channel: "github".to_string(),
                message: "invalid token".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_adapter() -> GitHubAdapter {
        GitHubAdapter::new(
            "ghp_test-token-123".to_string(),
            "humancto".to_string(),
            "punch".to_string(),
            "webhook-secret-456".to_string(),
        )
    }

    #[test]
    fn test_github_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "github");
        assert_eq!(adapter.platform(), ChannelPlatform::GitHub);
    }

    #[test]
    fn test_verify_webhook_signature_valid() {
        let adapter = make_adapter();
        let body = b"test payload body";

        // Compute expected signature
        let mut mac = HmacSha256::new_from_slice(b"webhook-secret-456").unwrap();
        mac.update(body);
        let expected = mac.finalize().into_bytes();
        let signature = format!("sha256={}", hex_encode(&expected));

        assert!(adapter.verify_webhook_signature(&signature, body));
    }

    #[test]
    fn test_verify_webhook_signature_invalid() {
        let adapter = make_adapter();
        let body = b"test payload body";
        let bad_signature =
            "sha256=0000000000000000000000000000000000000000000000000000000000000000";

        assert!(!adapter.verify_webhook_signature(bad_signature, body));
    }

    #[test]
    fn test_verify_webhook_signature_bad_prefix() {
        let adapter = make_adapter();
        assert!(!adapter.verify_webhook_signature("md5=abc", b"body"));
    }

    #[test]
    fn test_parse_issue_comment_webhook() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "action": "created",
            "issue": {
                "number": 42,
                "title": "Bug: something is broken"
            },
            "comment": {
                "id": 12345,
                "user": {
                    "login": "alice",
                    "id": 67890
                },
                "body": "@punch-bot please investigate this",
                "created_at": "2024-01-15T12:00:00Z"
            }
        });

        let msg = adapter
            .parse_webhook_payload("issue_comment", &payload)
            .unwrap();
        assert_eq!(msg.platform, ChannelPlatform::GitHub);
        assert_eq!(msg.user_id, "67890");
        assert_eq!(msg.display_name, "alice");
        assert_eq!(msg.text, "@punch-bot please investigate this");
        assert_eq!(msg.channel_id, "42");
        assert_eq!(msg.platform_message_id, "12345");
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_pr_review_comment_webhook() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "action": "created",
            "pull_request": {
                "number": 99
            },
            "comment": {
                "id": 54321,
                "user": {
                    "login": "bob",
                    "id": 11111
                },
                "body": "Can you refactor this?",
                "created_at": "2024-01-15T14:00:00Z",
                "path": "src/main.rs"
            }
        });

        let msg = adapter
            .parse_webhook_payload("pull_request_review_comment", &payload)
            .unwrap();
        assert_eq!(msg.channel_id, "99");
        assert_eq!(msg.display_name, "bob");
        assert_eq!(
            msg.metadata.get("file_path").unwrap(),
            &serde_json::Value::String("src/main.rs".to_string())
        );
    }

    #[test]
    fn test_parse_webhook_non_created_action() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "action": "deleted",
            "issue": { "number": 42, "title": "Test" },
            "comment": {
                "id": 12345,
                "user": { "login": "alice", "id": 67890 },
                "body": "deleted comment",
                "created_at": "2024-01-15T12:00:00Z"
            }
        });

        assert!(
            adapter
                .parse_webhook_payload("issue_comment", &payload)
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_github_adapter_start_stop() {
        let adapter = make_adapter();

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        let status = adapter.status();
        assert!(status.connected);
        assert!(status.started_at.is_some());

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }

    #[test]
    fn test_verify_webhook_signature_empty_body() {
        let adapter = make_adapter();
        let mut mac = HmacSha256::new_from_slice(b"webhook-secret-456").unwrap();
        mac.update(b"");
        let sig = format!("sha256={}", hex_encode(&mac.finalize().into_bytes()));
        assert!(adapter.verify_webhook_signature(&sig, b""));
    }

    #[test]
    fn test_verify_webhook_signature_no_prefix() {
        let adapter = make_adapter();
        assert!(!adapter.verify_webhook_signature("abc123", b"body"));
    }

    #[test]
    fn test_parse_unknown_event_type() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "action": "created",
            "comment": { "id": 1, "user": {"login": "a", "id": 1}, "body": "x", "created_at": "2024-01-01T00:00:00Z" }
        });
        assert!(adapter.parse_webhook_payload("push", &payload).is_none());
    }

    #[test]
    fn test_parse_issue_comment_empty_body() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "action": "created",
            "issue": { "number": 1, "title": "T" },
            "comment": { "id": 1, "user": {"login": "a", "id": 1}, "body": "", "created_at": "2024-01-01T00:00:00Z" }
        });
        assert!(
            adapter
                .parse_webhook_payload("issue_comment", &payload)
                .is_none()
        );
    }

    #[test]
    fn test_parse_pr_review_comment_empty_body() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "action": "created",
            "pull_request": { "number": 1 },
            "comment": { "id": 1, "user": {"login": "a", "id": 1}, "body": "", "created_at": "2024-01-01T00:00:00Z" }
        });
        assert!(
            adapter
                .parse_webhook_payload("pull_request_review_comment", &payload)
                .is_none()
        );
    }

    #[test]
    fn test_parse_issue_comment_metadata() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "action": "created",
            "issue": { "number": 7, "title": "My Issue" },
            "comment": { "id": 999, "user": {"login": "x", "id": 5}, "body": "hi", "created_at": "2024-01-01T00:00:00Z" }
        });
        let msg = adapter
            .parse_webhook_payload("issue_comment", &payload)
            .unwrap();
        assert_eq!(msg.metadata.get("event_type").unwrap(), "issue_comment");
        assert_eq!(
            msg.metadata.get("issue_number").unwrap(),
            &serde_json::json!(7)
        );
        assert_eq!(msg.metadata.get("issue_title").unwrap(), "My Issue");
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
    }

    #[test]
    fn test_constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"hello", b"hi"));
    }

    #[test]
    fn test_constant_time_eq_different_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }
}
