//! # punch-extensions
//!
//! MCP templates, credential vault, and WASM plugin sandbox for the
//! Punch Agent Combat System.
//!
//! Provides [`McpTemplate`] for defining MCP server configurations,
//! [`CredentialVault`] for securely storing and retrieving secrets using
//! AES-256-GCM encryption, and a [`plugin`] module for loading and
//! executing imported techniques in a sandboxed arena.

pub mod plugin;
pub mod wasm_runtime;

use std::collections::HashMap;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::prelude::*;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::info;
use zeroize::Zeroize;

use punch_types::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// MCP Templates
// ---------------------------------------------------------------------------

/// A template for an MCP (Model Context Protocol) server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTemplate {
    /// Human-readable name for this MCP server template.
    pub name: String,
    /// Command to start the MCP server.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Human-readable description.
    pub description: String,
    /// Environment variables required by this server (name -> description).
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Credential Vault
// ---------------------------------------------------------------------------

/// An encrypted credential entry.
#[derive(Clone, Serialize, Deserialize)]
struct EncryptedEntry {
    /// Base64-encoded ciphertext.
    ciphertext: String,
    /// Base64-encoded 12-byte nonce.
    nonce: String,
}

/// A vault for securely storing credentials using AES-256-GCM encryption.
///
/// The encryption key is held in memory and zeroized on drop.
pub struct CredentialVault {
    /// The 256-bit encryption key.
    key: Vec<u8>,
    /// Encrypted entries keyed by credential name.
    entries: HashMap<String, EncryptedEntry>,
}

impl CredentialVault {
    /// Create a new vault with a random encryption key.
    pub fn new() -> Self {
        let mut key = vec![0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self {
            key,
            entries: HashMap::new(),
        }
    }

    /// Create a new vault with a specific key (must be exactly 32 bytes).
    pub fn with_key(key: Vec<u8>) -> PunchResult<Self> {
        if key.len() != 32 {
            return Err(PunchError::Config(
                "credential vault key must be exactly 32 bytes".to_string(),
            ));
        }
        Ok(Self {
            key,
            entries: HashMap::new(),
        })
    }

    /// Encrypt and store a credential.
    pub fn store(&mut self, key: &str, value: &str) -> PunchResult<()> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| PunchError::Internal(format!("failed to create cipher: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, value.as_bytes())
            .map_err(|e| PunchError::Internal(format!("encryption failed: {e}")))?;

        let entry = EncryptedEntry {
            ciphertext: BASE64_STANDARD.encode(&ciphertext),
            nonce: BASE64_STANDARD.encode(nonce_bytes),
        };

        info!(key = %key, "stored credential in vault");
        self.entries.insert(key.to_string(), entry);
        Ok(())
    }

    /// Decrypt and retrieve a credential.
    pub fn retrieve(&self, key: &str) -> PunchResult<Option<String>> {
        let entry = match self.entries.get(key) {
            Some(e) => e,
            None => return Ok(None),
        };

        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| PunchError::Internal(format!("failed to create cipher: {e}")))?;

        let ciphertext = BASE64_STANDARD
            .decode(&entry.ciphertext)
            .map_err(|e| PunchError::Internal(format!("failed to decode ciphertext: {e}")))?;

        let nonce_bytes = BASE64_STANDARD
            .decode(&entry.nonce)
            .map_err(|e| PunchError::Internal(format!("failed to decode nonce: {e}")))?;

        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| PunchError::Internal(format!("decryption failed: {e}")))?;

        String::from_utf8(plaintext)
            .map(Some)
            .map_err(|e| PunchError::Internal(format!("decrypted value is not valid UTF-8: {e}")))
    }

    /// Delete a credential from the vault.
    pub fn delete(&mut self, key: &str) -> bool {
        let removed = self.entries.remove(key).is_some();
        if removed {
            info!(key = %key, "deleted credential from vault");
        }
        removed
    }

    /// List all credential keys in the vault.
    pub fn list_keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

impl Default for CredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CredentialVault {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_vault_new() {
        let vault = CredentialVault::new();
        assert!(vault.list_keys().is_empty());
    }

    #[test]
    fn test_credential_vault_default() {
        let vault = CredentialVault::default();
        assert!(vault.list_keys().is_empty());
    }

    #[test]
    fn test_with_key_valid() {
        let key = vec![0u8; 32];
        let vault = CredentialVault::with_key(key);
        assert!(vault.is_ok());
    }

    #[test]
    fn test_with_key_invalid_length() {
        let key = vec![0u8; 16];
        let vault = CredentialVault::with_key(key);
        assert!(vault.is_err());
    }

    #[test]
    fn test_store_and_retrieve() {
        let mut vault = CredentialVault::new();
        vault.store("api_key", "sk-secret-123").unwrap();
        let retrieved = vault.retrieve("api_key").unwrap();
        assert_eq!(retrieved, Some("sk-secret-123".to_string()));
    }

    #[test]
    fn test_retrieve_nonexistent() {
        let vault = CredentialVault::new();
        let result = vault.retrieve("missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_store_multiple_credentials() {
        let mut vault = CredentialVault::new();
        vault.store("key1", "value1").unwrap();
        vault.store("key2", "value2").unwrap();
        vault.store("key3", "value3").unwrap();

        assert_eq!(vault.list_keys().len(), 3);
        assert_eq!(vault.retrieve("key1").unwrap(), Some("value1".to_string()));
        assert_eq!(vault.retrieve("key2").unwrap(), Some("value2".to_string()));
        assert_eq!(vault.retrieve("key3").unwrap(), Some("value3".to_string()));
    }

    #[test]
    fn test_store_overwrites_existing() {
        let mut vault = CredentialVault::new();
        vault.store("key", "original").unwrap();
        vault.store("key", "updated").unwrap();

        assert_eq!(vault.retrieve("key").unwrap(), Some("updated".to_string()));
        assert_eq!(vault.list_keys().len(), 1);
    }

    #[test]
    fn test_delete_existing() {
        let mut vault = CredentialVault::new();
        vault.store("key", "value").unwrap();
        assert!(vault.delete("key"));
        assert!(vault.retrieve("key").unwrap().is_none());
        assert!(vault.list_keys().is_empty());
    }

    #[test]
    fn test_delete_nonexistent() {
        let mut vault = CredentialVault::new();
        assert!(!vault.delete("nonexistent"));
    }

    #[test]
    fn test_list_keys() {
        let mut vault = CredentialVault::new();
        vault.store("alpha", "a").unwrap();
        vault.store("beta", "b").unwrap();

        let mut keys = vault.list_keys();
        keys.sort();
        assert_eq!(keys, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_store_empty_value() {
        let mut vault = CredentialVault::new();
        vault.store("empty", "").unwrap();
        assert_eq!(vault.retrieve("empty").unwrap(), Some(String::new()));
    }

    #[test]
    fn test_store_unicode_value() {
        let mut vault = CredentialVault::new();
        vault.store("unicode", "日本語テスト 🔑").unwrap();
        assert_eq!(
            vault.retrieve("unicode").unwrap(),
            Some("日本語テスト 🔑".to_string())
        );
    }

    #[test]
    fn test_mcp_template_serde() {
        let template = McpTemplate {
            name: "test-server".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@test/server".to_string()],
            description: "A test MCP server".to_string(),
            env_vars: HashMap::new(),
        };
        let json = serde_json::to_string(&template).unwrap();
        let restored: McpTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test-server");
        assert_eq!(restored.args.len(), 2);
    }

    #[test]
    fn test_mcp_template_with_env_vars() {
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "Your API key".to_string());
        let template = McpTemplate {
            name: "env-server".to_string(),
            command: "cmd".to_string(),
            args: vec![],
            description: "desc".to_string(),
            env_vars: env,
        };
        let json = serde_json::to_string(&template).unwrap();
        let restored: McpTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.env_vars.len(), 1);
        assert!(restored.env_vars.contains_key("API_KEY"));
    }
}
