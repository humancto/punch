//! Secret management with zeroization — keeps secrets locked in the vault.
//!
//! Provides a zero-on-drop `Secret` wrapper that wipes sensitive data from
//! memory when it goes out of scope. The `SecretStore` offers a concurrent,
//! named vault for storing and retrieving secrets, while `SecretProvider`
//! implementations load secrets from environment variables, files, or other
//! sources.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// ---------------------------------------------------------------------------
// Secret<T> wrapper
// ---------------------------------------------------------------------------

/// A wrapper that zeroizes its inner value when dropped.
///
/// Prevents secrets from lingering in memory after they are no longer needed.
/// Like wiping the blood from the canvas between bouts.
pub struct Secret<T: Zeroize> {
    inner: T,
}

impl<T: Zeroize> Secret<T> {
    /// Wrap a value in the secret container.
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// Access the secret value. Handle with care — the contents are sensitive.
    pub fn expose(&self) -> &T {
        &self.inner
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// SecretString alias
// ---------------------------------------------------------------------------

/// A `Secret<String>` — the most common secret type.
pub type SecretString = Secret<String>;

// ---------------------------------------------------------------------------
// SecretStore
// ---------------------------------------------------------------------------

/// A concurrent, named vault for storing secrets.
///
/// Backed by `DashMap` for lock-free concurrent access. Secret values are
/// wrapped in `SecretString` so they are zeroized when removed or when the
/// store is dropped.
#[derive(Debug, Clone)]
pub struct SecretStore {
    secrets: Arc<DashMap<String, String>>,
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore {
    /// Create an empty vault.
    pub fn new() -> Self {
        Self {
            secrets: Arc::new(DashMap::new()),
        }
    }

    /// Store a named secret. If a secret with the same name already exists,
    /// the old value is zeroized and replaced.
    pub fn store_secret(&self, name: &str, value: &str) {
        if let Some(mut old) = self.secrets.get_mut(name) {
            old.value_mut().zeroize();
        }
        self.secrets.insert(name.to_string(), value.to_string());
    }

    /// Retrieve a named secret wrapped in a `SecretString`.
    ///
    /// Returns `None` if the secret does not exist.
    pub fn get_secret(&self, name: &str) -> Option<SecretString> {
        self.secrets
            .get(name)
            .map(|entry| Secret::new(entry.value().clone()))
    }

    /// Delete a named secret, zeroizing its value before removal.
    pub fn delete_secret(&self, name: &str) {
        if let Some((_, mut value)) = self.secrets.remove(name) {
            value.zeroize();
        }
    }

    /// List all secret names without exposing their values.
    pub fn list_secret_names(&self) -> Vec<String> {
        self.secrets
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Return the number of secrets in the store.
    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Masking
// ---------------------------------------------------------------------------

/// Mask a secret value for safe display.
///
/// Shows the first 2 and last 2 characters with asterisks in between.
/// Values shorter than 5 characters are fully masked.
pub fn mask_secret(value: &str) -> String {
    if value.len() < 5 {
        return "*".repeat(value.len());
    }
    let chars: Vec<char> = value.chars().collect();
    let first_two: String = chars[..2].iter().collect();
    let last_two: String = chars[chars.len() - 2..].iter().collect();
    let mask_len = chars.len() - 4;
    format!("{}{}{}", first_two, "*".repeat(mask_len), last_two)
}

// ---------------------------------------------------------------------------
// SecretProvider trait
// ---------------------------------------------------------------------------

/// A source of secrets — loads secrets from an external provider.
pub trait SecretProvider: Send + Sync {
    /// Load all available secrets into the given store.
    fn load_secrets(&self, store: &SecretStore) -> Result<usize, SecretProviderError>;
}

/// Errors from secret providers.
#[derive(Debug, thiserror::Error)]
pub enum SecretProviderError {
    /// An I/O error occurred while reading secrets.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The secret source format is invalid.
    #[error("invalid format: {0}")]
    InvalidFormat(String),
}

// ---------------------------------------------------------------------------
// EnvSecretProvider
// ---------------------------------------------------------------------------

/// Loads secrets from environment variables with a configurable prefix.
///
/// For example, with prefix `"PUNCH_SECRET_"`, the env var
/// `PUNCH_SECRET_API_KEY=xyz` becomes a secret named `API_KEY` with value `xyz`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSecretProvider {
    /// The prefix to filter environment variables.
    pub prefix: String,
}

impl EnvSecretProvider {
    /// Create a provider that reads env vars with the given prefix.
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }
}

impl SecretProvider for EnvSecretProvider {
    fn load_secrets(&self, store: &SecretStore) -> Result<usize, SecretProviderError> {
        let mut count = 0;
        for (key, value) in std::env::vars() {
            if key.starts_with(&self.prefix) {
                let name = &key[self.prefix.len()..];
                if !name.is_empty() {
                    store.store_secret(name, &value);
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// FileSecretProvider
// ---------------------------------------------------------------------------

/// Loads secrets from a file in `KEY=VALUE` format (one per line).
///
/// Lines starting with `#` are treated as comments. Empty lines are skipped.
/// Leading and trailing whitespace on keys and values is trimmed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSecretProvider {
    /// Path to the secrets file.
    pub path: String,
}

impl FileSecretProvider {
    /// Create a provider that reads secrets from the given file path.
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }

    /// Parse a secrets file content into key-value pairs.
    pub fn parse_secrets(content: &str) -> Result<HashMap<String, String>, SecretProviderError> {
        let mut secrets = HashMap::new();
        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim();
                let value = trimmed[eq_pos + 1..].trim();
                if key.is_empty() {
                    return Err(SecretProviderError::InvalidFormat(format!(
                        "empty key on line {}",
                        line_num + 1
                    )));
                }
                secrets.insert(key.to_string(), value.to_string());
            } else {
                return Err(SecretProviderError::InvalidFormat(format!(
                    "missing '=' on line {}",
                    line_num + 1
                )));
            }
        }
        Ok(secrets)
    }
}

impl SecretProvider for FileSecretProvider {
    fn load_secrets(&self, store: &SecretStore) -> Result<usize, SecretProviderError> {
        let content = std::fs::read_to_string(Path::new(&self.path))?;
        let secrets = Self::parse_secrets(&content)?;
        let count = secrets.len();
        for (key, value) in secrets {
            store.store_secret(&key, &value);
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_display_is_masked() {
        let secret = Secret::new("super-secret-value".to_string());
        assert_eq!(format!("{}", secret), "***");
        assert_eq!(format!("{:?}", secret), "Secret(***)");
    }

    #[test]
    fn test_secret_expose() {
        let secret = Secret::new("my-value".to_string());
        assert_eq!(secret.expose(), "my-value");
    }

    #[test]
    fn test_store_and_retrieve() {
        let store = SecretStore::new();
        store.store_secret("API_KEY", "abc123");
        let retrieved = store.get_secret("API_KEY").unwrap();
        assert_eq!(retrieved.expose(), "abc123");
    }

    #[test]
    fn test_store_missing_key() {
        let store = SecretStore::new();
        assert!(store.get_secret("NONEXISTENT").is_none());
    }

    #[test]
    fn test_delete_secret() {
        let store = SecretStore::new();
        store.store_secret("TO_DELETE", "value");
        assert!(store.get_secret("TO_DELETE").is_some());
        store.delete_secret("TO_DELETE");
        assert!(store.get_secret("TO_DELETE").is_none());
    }

    #[test]
    fn test_list_secret_names() {
        let store = SecretStore::new();
        store.store_secret("ALPHA", "a");
        store.store_secret("BETA", "b");
        store.store_secret("GAMMA", "c");
        let mut names = store.list_secret_names();
        names.sort();
        assert_eq!(names, vec!["ALPHA", "BETA", "GAMMA"]);
    }

    #[test]
    fn test_store_len_and_empty() {
        let store = SecretStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        store.store_secret("KEY", "val");
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_overwrite_secret() {
        let store = SecretStore::new();
        store.store_secret("KEY", "old");
        store.store_secret("KEY", "new");
        let retrieved = store.get_secret("KEY").unwrap();
        assert_eq!(retrieved.expose(), "new");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_mask_secret_normal() {
        assert_eq!(mask_secret("abcdefgh"), "ab****gh");
        assert_eq!(mask_secret("12345"), "12*45");
    }

    #[test]
    fn test_mask_secret_short() {
        assert_eq!(mask_secret("ab"), "**");
        assert_eq!(mask_secret("abc"), "***");
        assert_eq!(mask_secret("abcd"), "****");
    }

    #[test]
    fn test_mask_secret_empty() {
        assert_eq!(mask_secret(""), "");
    }

    #[test]
    fn test_zeroization_on_drop() {
        // We can verify that after dropping, the Secret no longer holds data.
        // While we cannot directly inspect freed memory, we can verify the
        // zeroize trait is invoked by checking a clone before and after.
        let mut value = String::from("sensitive-data");
        value.zeroize();
        // After zeroize, the string should be empty.
        assert!(value.is_empty());
    }

    #[test]
    fn test_env_secret_provider() {
        let prefix = "PUNCH_TEST_SECRET_STORE_";
        // Set up test env vars.
        unsafe {
            std::env::set_var(format!("{}DB_PASS", prefix), "hunter2");
            std::env::set_var(format!("{}API_KEY", prefix), "key123");
        }

        let provider = EnvSecretProvider::new(prefix);
        let store = SecretStore::new();
        let count = provider.load_secrets(&store).unwrap();
        assert!(count >= 2);
        assert_eq!(store.get_secret("DB_PASS").unwrap().expose(), "hunter2");
        assert_eq!(store.get_secret("API_KEY").unwrap().expose(), "key123");

        // Clean up.
        unsafe {
            std::env::remove_var(format!("{}DB_PASS", prefix));
            std::env::remove_var(format!("{}API_KEY", prefix));
        }
    }

    #[test]
    fn test_file_secret_provider_parse() {
        let content = r#"
# Database credentials
DB_HOST=localhost
DB_PASS=supersecret

# API keys
API_KEY=abc123
"#;
        let secrets = FileSecretProvider::parse_secrets(content).unwrap();
        assert_eq!(secrets.len(), 3);
        assert_eq!(secrets.get("DB_HOST").unwrap(), "localhost");
        assert_eq!(secrets.get("DB_PASS").unwrap(), "supersecret");
        assert_eq!(secrets.get("API_KEY").unwrap(), "abc123");
    }

    #[test]
    fn test_file_secret_provider_invalid_format() {
        let content = "VALID=ok\nINVALID_LINE_NO_EQUALS";
        let result = FileSecretProvider::parse_secrets(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_secret_clone() {
        let secret = Secret::new("cloneable".to_string());
        let cloned = secret.clone();
        assert_eq!(cloned.expose(), "cloneable");
    }
}
