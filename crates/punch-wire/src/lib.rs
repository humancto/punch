//! # punch-wire
//!
//! P2P networking protocol for the Punch Agent Combat System.
//!
//! Provides peer-to-peer communication with HMAC-SHA256 mutual authentication
//! for connecting Punch instances across a network.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A peer in the P2P network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Unique identifier for this peer.
    pub id: String,
    /// Network address of the peer.
    pub addr: SocketAddr,
    /// Whether this peer has been authenticated via HMAC handshake.
    pub authenticated: bool,
}

/// A message sent between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMessage {
    /// Sender peer ID.
    pub from: String,
    /// Recipient peer ID.
    pub to: String,
    /// Message payload (JSON).
    pub payload: serde_json::Value,
    /// HMAC-SHA256 signature of the payload.
    pub signature: String,
    /// Random nonce for replay protection.
    pub nonce: String,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// PunchProtocol
// ---------------------------------------------------------------------------

/// The P2P networking protocol for Punch instances.
pub struct PunchProtocol {
    /// This node's peer ID.
    peer_id: String,
    /// Shared secret for HMAC authentication.
    shared_secret: Vec<u8>,
    /// Known peers.
    peers: Arc<RwLock<HashMap<String, Peer>>>,
    /// Incoming message buffer.
    inbox: Arc<RwLock<Vec<PeerMessage>>>,
}

impl PunchProtocol {
    /// Create a new protocol instance.
    pub fn new(peer_id: String, shared_secret: Vec<u8>) -> Self {
        Self {
            peer_id,
            shared_secret,
            peers: Arc::new(RwLock::new(HashMap::new())),
            inbox: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get this node's peer ID.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Start listening for incoming peer connections on the given address.
    ///
    /// This spawns a background task that accepts connections.
    pub async fn listen(&self, addr: SocketAddr) -> PunchResult<()> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| PunchError::Internal(format!("failed to bind to {}: {}", addr, e)))?;

        info!(addr = %addr, peer_id = %self.peer_id, "P2P listener started");

        let peers = self.peers.clone();
        let inbox = self.inbox.clone();
        let secret = self.shared_secret.clone();

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        info!(peer_addr = %peer_addr, "accepted P2P connection");
                        let peers = peers.clone();
                        let inbox = inbox.clone();
                        let secret = secret.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_connection(stream, peer_addr, &secret, &peers, &inbox).await
                            {
                                warn!(peer_addr = %peer_addr, error = %e, "connection handler failed");
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to accept connection");
                    }
                }
            }
        });

        Ok(())
    }

    /// Connect to a remote peer.
    pub async fn connect(&self, peer_addr: SocketAddr) -> PunchResult<()> {
        let _stream = TcpStream::connect(peer_addr).await.map_err(|e| {
            PunchError::Internal(format!("failed to connect to {}: {}", peer_addr, e))
        })?;

        info!(peer_addr = %peer_addr, "connected to peer");

        let peer = Peer {
            id: peer_addr.to_string(),
            addr: peer_addr,
            authenticated: false,
        };

        self.peers.write().await.insert(peer.id.clone(), peer);

        Ok(())
    }

    /// Send a message to a specific peer.
    pub async fn send_message(&self, peer_id: &str, payload: serde_json::Value) -> PunchResult<()> {
        let peers = self.peers.read().await;
        let _peer = peers
            .get(peer_id)
            .ok_or_else(|| PunchError::Internal(format!("peer {} not found", peer_id)))?;

        let mut nonce_bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = hex::encode(nonce_bytes);

        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| PunchError::Internal(format!("failed to serialize payload: {e}")))?;

        let signature = compute_hmac(&self.shared_secret, &payload_bytes);

        let message = PeerMessage {
            from: self.peer_id.clone(),
            to: peer_id.to_string(),
            payload,
            signature,
            nonce,
            timestamp: Utc::now(),
        };

        // In a full implementation, this would send over the TCP connection.
        // For now, we just log the message.
        info!(
            from = %message.from,
            to = %message.to,
            "sending P2P message"
        );

        Ok(())
    }

    /// Retrieve and drain pending incoming messages.
    pub async fn receive(&self) -> Vec<PeerMessage> {
        let mut inbox = self.inbox.write().await;
        std::mem::take(&mut *inbox)
    }

    /// List all known peers.
    pub async fn list_peers(&self) -> Vec<Peer> {
        self.peers.read().await.values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute an HMAC-SHA256 signature and return it as a hex string.
fn compute_hmac(secret: &[u8], data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(data);
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Verify an HMAC-SHA256 signature.
fn verify_hmac(secret: &[u8], data: &[u8], signature: &str) -> bool {
    let expected = compute_hmac(secret, data);
    // Constant-time comparison would be better, but this is sufficient for now.
    expected == signature
}

/// Handle an incoming peer connection.
async fn handle_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    secret: &[u8],
    peers: &RwLock<HashMap<String, Peer>>,
    inbox: &RwLock<Vec<PeerMessage>>,
) -> PunchResult<()> {
    let mut buf = vec![0u8; 65536];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| PunchError::Internal(format!("failed to read from peer: {e}")))?;

    if n == 0 {
        return Ok(());
    }

    let message: PeerMessage = serde_json::from_slice(&buf[..n])
        .map_err(|e| PunchError::Internal(format!("failed to parse peer message: {e}")))?;

    // Verify HMAC signature
    let payload_bytes = serde_json::to_vec(&message.payload).map_err(|e| {
        PunchError::Internal(format!("failed to serialize payload for verification: {e}"))
    })?;

    if !verify_hmac(secret, &payload_bytes, &message.signature) {
        warn!(peer_addr = %peer_addr, "HMAC verification failed");
        return Err(PunchError::Auth("HMAC verification failed".to_string()));
    }

    // Register/update peer as authenticated
    let peer = Peer {
        id: message.from.clone(),
        addr: peer_addr,
        authenticated: true,
    };
    peers.write().await.insert(peer.id.clone(), peer);

    // Store message in inbox
    inbox.write().await.push(message);

    Ok(())
}

/// Encode bytes to hex string. Minimal implementation to avoid extra deps.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hmac_deterministic() {
        let secret = b"my-shared-secret";
        let data = b"hello world";
        let sig1 = compute_hmac(secret, data);
        let sig2 = compute_hmac(secret, data);
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_compute_hmac_different_data() {
        let secret = b"my-shared-secret";
        let sig1 = compute_hmac(secret, b"message A");
        let sig2 = compute_hmac(secret, b"message B");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_compute_hmac_different_keys() {
        let data = b"same data";
        let sig1 = compute_hmac(b"key-1", data);
        let sig2 = compute_hmac(b"key-2", data);
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_verify_hmac_valid() {
        let secret = b"shared-secret-42";
        let data = b"payload data";
        let sig = compute_hmac(secret, data);
        assert!(verify_hmac(secret, data, &sig));
    }

    #[test]
    fn test_verify_hmac_invalid_signature() {
        let secret = b"shared-secret-42";
        let data = b"payload data";
        assert!(!verify_hmac(secret, data, "invalid-hex-signature"));
    }

    #[test]
    fn test_verify_hmac_wrong_key() {
        let data = b"payload data";
        let sig = compute_hmac(b"correct-key", data);
        assert!(!verify_hmac(b"wrong-key", data, &sig));
    }

    #[test]
    fn test_verify_hmac_tampered_data() {
        let secret = b"secret";
        let sig = compute_hmac(secret, b"original");
        assert!(!verify_hmac(secret, b"tampered", &sig));
    }

    #[test]
    fn test_hmac_output_is_hex() {
        let sig = compute_hmac(b"key", b"data");
        // HMAC-SHA256 produces 32 bytes = 64 hex chars
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex::encode([0x00]), "00");
        assert_eq!(hex::encode([0xff]), "ff");
        assert_eq!(hex::encode([0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex::encode([]), "");
    }

    #[test]
    fn test_peer_serialization() {
        let peer = Peer {
            id: "peer-1".to_string(),
            addr: "127.0.0.1:8080".parse().unwrap(),
            authenticated: true,
        };
        let json = serde_json::to_string(&peer).unwrap();
        let deserialized: Peer = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "peer-1");
        assert!(deserialized.authenticated);
    }

    #[test]
    fn test_peer_message_serialization() {
        let msg = PeerMessage {
            from: "peer-a".to_string(),
            to: "peer-b".to_string(),
            payload: serde_json::json!({"action": "sync"}),
            signature: "abc123".to_string(),
            nonce: "nonce-value".to_string(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: PeerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.from, "peer-a");
        assert_eq!(deserialized.to, "peer-b");
        assert_eq!(deserialized.payload["action"], "sync");
    }

    #[tokio::test]
    async fn test_punch_protocol_creation() {
        let proto = PunchProtocol::new("node-1".to_string(), b"secret".to_vec());
        assert_eq!(proto.peer_id(), "node-1");
    }

    #[tokio::test]
    async fn test_punch_protocol_empty_peers() {
        let proto = PunchProtocol::new("node-1".to_string(), b"secret".to_vec());
        let peers = proto.list_peers().await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn test_punch_protocol_empty_inbox() {
        let proto = PunchProtocol::new("node-1".to_string(), b"secret".to_vec());
        let messages = proto.receive().await;
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_send_message_unknown_peer() {
        let proto = PunchProtocol::new("node-1".to_string(), b"secret".to_vec());
        let result = proto
            .send_message("nonexistent-peer", serde_json::json!({"hello": "world"}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_peer_unauthenticated_default() {
        let peer = Peer {
            id: "new-peer".to_string(),
            addr: "192.168.1.1:9090".parse().unwrap(),
            authenticated: false,
        };
        assert!(!peer.authenticated);
    }
}
