//! # punch-extensions
//!
//! MCP templates and credential vault for the Punch Agent Combat System.
//!
//! Provides [`McpTemplate`] for defining MCP server configurations and
//! [`CredentialVault`] for securely storing and retrieving secrets using
//! AES-256-GCM encryption.

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
            .map(|s| Some(s))
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
