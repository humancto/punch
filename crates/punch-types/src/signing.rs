//! Ed25519 manifest signing and verification.
//!
//! Every manifest that enters the ring can be cryptographically signed to
//! guarantee authenticity and integrity. A fighter's identity is bound to
//! an Ed25519 keypair — the signing key stays in the corner, while the
//! verifying key can be distributed to anyone who needs to validate a
//! manifest before it lands.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during signing or verification.
#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    /// The hex-encoded key could not be decoded.
    #[error("invalid hex encoding: {0}")]
    HexDecode(String),

    /// The key bytes have an invalid length or format.
    #[error("invalid key format: {0}")]
    InvalidKey(String),

    /// The signature bytes have an invalid length or format.
    #[error("invalid signature format: {0}")]
    InvalidSignature(String),

    /// Signature verification failed.
    #[error("signature verification failed")]
    VerificationFailed,
}

// ---------------------------------------------------------------------------
// SigningKeyPair
// ---------------------------------------------------------------------------

/// An Ed25519 signing keypair — the fighter's secret identity in the ring.
///
/// Wraps `ed25519_dalek::SigningKey` and provides convenient methods for
/// signing manifests and serializing keys to hex strings.
pub struct SigningKeyPair {
    inner: SigningKey,
}

impl std::fmt::Debug for SigningKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKeyPair")
            .field("public_key", &self.verifying_key_hex())
            .finish()
    }
}

impl SigningKeyPair {
    /// Create a `SigningKeyPair` from raw secret key bytes (32 bytes).
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            inner: SigningKey::from_bytes(bytes),
        }
    }

    /// Reconstruct a `SigningKeyPair` from a hex-encoded secret key.
    pub fn from_hex(hex: &str) -> Result<Self, SigningError> {
        let bytes = hex_decode(hex)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| SigningError::InvalidKey("secret key must be 32 bytes".into()))?;
        Ok(Self::from_bytes(&arr))
    }

    /// Return the secret key as a hex-encoded string.
    pub fn secret_key_hex(&self) -> String {
        hex_encode(self.inner.as_bytes())
    }

    /// Return the corresponding verifying (public) key.
    pub fn verifying_key(&self) -> VerifyingKey {
        self.inner.verifying_key()
    }

    /// Return the verifying key as a hex-encoded string.
    pub fn verifying_key_hex(&self) -> String {
        hex_encode(self.verifying_key().as_bytes())
    }

    /// Sign arbitrary bytes and return the signature.
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.inner.sign(message)
    }
}

// ---------------------------------------------------------------------------
// Key generation
// ---------------------------------------------------------------------------

/// Generate a fresh Ed25519 keypair using OS-level randomness.
///
/// Returns the signing keypair and the corresponding verifying key.
pub fn generate_keypair() -> (SigningKeyPair, VerifyingKey) {
    let mut csprng = rand::rngs::OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();
    (SigningKeyPair { inner: signing_key }, verifying_key)
}

// ---------------------------------------------------------------------------
// Manifest signing helpers
// ---------------------------------------------------------------------------

/// Sign manifest bytes and return a hex-encoded 64-byte signature.
pub fn sign_manifest(keypair: &SigningKeyPair, manifest_bytes: &[u8]) -> String {
    let sig = keypair.sign(manifest_bytes);
    hex_encode(&sig.to_bytes())
}

/// Verify a hex-encoded signature against manifest bytes and a verifying key.
pub fn verify_manifest(
    verifying_key: &VerifyingKey,
    manifest_bytes: &[u8],
    signature_hex: &str,
) -> Result<bool, SigningError> {
    let sig_bytes = hex_decode(signature_hex)?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| SigningError::InvalidSignature("signature must be 64 bytes".into()))?;
    let signature = Signature::from_bytes(&sig_arr);
    Ok(verifying_key.verify(manifest_bytes, &signature).is_ok())
}

// ---------------------------------------------------------------------------
// SignedManifest
// ---------------------------------------------------------------------------

/// A manifest bundled with its signature and the signer's public key.
///
/// Everything needed to verify authenticity in a single struct — ready to
/// be serialized and transmitted across the ring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    /// The raw manifest bytes.
    pub manifest: Vec<u8>,
    /// Hex-encoded Ed25519 signature (128 hex chars = 64 bytes).
    pub signature: String,
    /// Hex-encoded Ed25519 public key (64 hex chars = 32 bytes).
    pub public_key: String,
}

/// Sign manifest bytes and wrap them into a `SignedManifest`.
pub fn sign_and_wrap(keypair: &SigningKeyPair, manifest: Vec<u8>) -> SignedManifest {
    let signature = sign_manifest(keypair, &manifest);
    let public_key = keypair.verifying_key_hex();
    SignedManifest {
        manifest,
        signature,
        public_key,
    }
}

/// Verify a `SignedManifest` — reconstruct the public key from the embedded
/// hex string and check the signature against the manifest bytes.
pub fn verify_signed_manifest(signed: &SignedManifest) -> Result<bool, SigningError> {
    let pk_bytes = hex_decode(&signed.public_key)?;
    let pk_arr: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| SigningError::InvalidKey("public key must be 32 bytes".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|e| SigningError::InvalidKey(format!("invalid public key: {}", e)))?;
    verify_manifest(&verifying_key, &signed.manifest, &signed.signature)
}

// ---------------------------------------------------------------------------
// Verifying key from hex
// ---------------------------------------------------------------------------

/// Reconstruct a `VerifyingKey` from a hex-encoded string.
pub fn verifying_key_from_hex(hex: &str) -> Result<VerifyingKey, SigningError> {
    let bytes = hex_decode(hex)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SigningError::InvalidKey("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| SigningError::InvalidKey(format!("invalid public key: {}", e)))
}

// ---------------------------------------------------------------------------
// Hex helpers (no external dependency needed)
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, SigningError> {
    if !hex.len().is_multiple_of(2) {
        return Err(SigningError::HexDecode(
            "odd number of hex characters".into(),
        ));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| {
                SigningError::HexDecode(format!("invalid hex at position {}: {}", i, e))
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let (kp, vk) = generate_keypair();
        assert_eq!(kp.verifying_key(), vk);
    }

    #[test]
    fn test_sign_and_verify() {
        let (kp, vk) = generate_keypair();
        let manifest = b"hello manifest";
        let sig = sign_manifest(&kp, manifest);
        let valid = verify_manifest(&vk, manifest, &sig).unwrap();
        assert!(valid);
    }

    #[test]
    fn test_tamper_detection() {
        let (kp, vk) = generate_keypair();
        let manifest = b"original manifest";
        let sig = sign_manifest(&kp, manifest);
        let valid = verify_manifest(&vk, b"tampered manifest", &sig).unwrap();
        assert!(!valid, "tampered manifest should fail verification");
    }

    #[test]
    fn test_wrong_key_rejection() {
        let (kp1, _vk1) = generate_keypair();
        let (_kp2, vk2) = generate_keypair();
        let manifest = b"manifest for keypair 1";
        let sig = sign_manifest(&kp1, manifest);
        let valid = verify_manifest(&vk2, manifest, &sig).unwrap();
        assert!(!valid, "wrong verifying key should fail");
    }

    #[test]
    fn test_signed_manifest_roundtrip() {
        let (kp, _vk) = generate_keypair();
        let manifest = b"roundtrip manifest".to_vec();
        let signed = sign_and_wrap(&kp, manifest.clone());
        assert_eq!(signed.manifest, manifest);
        let valid = verify_signed_manifest(&signed).unwrap();
        assert!(valid);
    }

    #[test]
    fn test_signed_manifest_tampered() {
        let (kp, _vk) = generate_keypair();
        let manifest = b"original data".to_vec();
        let mut signed = sign_and_wrap(&kp, manifest);
        signed.manifest = b"tampered data".to_vec();
        let valid = verify_signed_manifest(&signed).unwrap();
        assert!(!valid);
    }

    #[test]
    fn test_key_hex_roundtrip() {
        let (kp, _vk) = generate_keypair();
        let secret_hex = kp.secret_key_hex();
        let restored = SigningKeyPair::from_hex(&secret_hex).unwrap();
        assert_eq!(restored.verifying_key(), kp.verifying_key());
    }

    #[test]
    fn test_verifying_key_hex_roundtrip() {
        let (_kp, vk) = generate_keypair();
        let hex = hex_encode(vk.as_bytes());
        let restored = verifying_key_from_hex(&hex).unwrap();
        assert_eq!(restored, vk);
    }

    #[test]
    fn test_invalid_hex_signature() {
        let (_kp, vk) = generate_keypair();
        let result = verify_manifest(&vk, b"data", "not_valid_hex!");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_hex_key() {
        let result = SigningKeyPair::from_hex("zzzz");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_manifest_signing() {
        let (kp, vk) = generate_keypair();
        let sig = sign_manifest(&kp, b"");
        let valid = verify_manifest(&vk, b"", &sig).unwrap();
        assert!(valid, "empty manifest should sign and verify");
    }

    #[test]
    fn test_large_manifest_signing() {
        let (kp, vk) = generate_keypair();
        let manifest = vec![0xABu8; 1_000_000];
        let sig = sign_manifest(&kp, &manifest);
        let valid = verify_manifest(&vk, &manifest, &sig).unwrap();
        assert!(valid, "large manifest should sign and verify");
    }

    #[test]
    fn test_signature_is_hex_encoded() {
        let (kp, _vk) = generate_keypair();
        let sig = sign_manifest(&kp, b"data");
        // 64 bytes = 128 hex characters
        assert_eq!(sig.len(), 128);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_signed_manifest_serialization() {
        let (kp, _vk) = generate_keypair();
        let signed = sign_and_wrap(&kp, b"json test".to_vec());
        let json = serde_json::to_string(&signed).unwrap();
        let deserialized: SignedManifest = serde_json::from_str(&json).unwrap();
        let valid = verify_signed_manifest(&deserialized).unwrap();
        assert!(valid, "deserialized signed manifest should verify");
    }
}
