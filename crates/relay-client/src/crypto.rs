//! End-to-end encryption for the relay control plane.
//!
//! After pairing, both peers derive a 32-byte session key from the pairing
//! material via HKDF-SHA256. Every `Envelope` is serialized, optionally
//! gzip-compressed (large payloads only; flagged via `EncryptedEnvelope::
//! encoding`), AEAD-encrypted (ChaCha20-Poly1305) with a fresh random nonce,
//! and wrapped in an `EncryptedEnvelope` for relay transit. The relay routes
//! by `to_device_id` only and never sees plaintext. The `to_device_id` is
//! bound as AEAD associated data so a ciphertext cannot be replayed against a
//! different routing target.

use anyhow::{Result, anyhow};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use std::io::{Read, Write};

use relay_protocol::{EncryptedEnvelope, Envelope, PROTO_VERSION};

/// Payload encoding marker stored on the wire in `EncryptedEnvelope.encoding`.
pub const ENCODING_GZIP: &str = "gzip";
/// Serialized envelopes above this size are gzip-compressed before
/// encryption. Below it, compression overhead outweighs the savings.
const COMPRESS_THRESHOLD_BYTES: usize = 1024;

/// 32-byte AEAD key derived from pairing material.
#[derive(Clone)]
pub struct SessionKey([u8; 32]);

impl SessionKey {
    /// Derive a session key from raw pairing key-exchange material via
    /// HKDF-SHA256. `salt` may be a stable relay/app identifier.
    pub fn derive(pairing_material: &[u8], salt: &[u8]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(salt), pairing_material);
        let mut okm = [0u8; 32];
        hk.expand(b"kodex-relay-e2e-v1", &mut okm)
            .expect("32-byte HKDF expand fits");
        SessionKey(okm)
    }

    fn as_key(&self) -> &Key {
        Key::from_slice(&self.0)
    }

    /// First 8 bytes, for cross-endpoint diagnostic comparison only.
    pub fn bytes_prefix(&self) -> [u8; 8] {
        self.0[..8].try_into().expect("32-byte key")
    }
}

const NONCE_LEN: usize = 12;

/// Encrypt a typed `Envelope` into a relay-routable `EncryptedEnvelope`.
/// `to_device_id` is the routing target and is bound as AEAD AAD.
/// Large payloads are gzip-compressed before encryption and the envelope's
/// `encoding` field flags that, so the receiver can invert it after decrypt.
pub fn encrypt(
    key: &SessionKey,
    to_device_id: &str,
    envelope: &Envelope,
) -> Result<EncryptedEnvelope> {
    let cipher = ChaCha20Poly1305::new(key.as_key());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let serialized = serde_json::to_vec(envelope)?;
    let (plaintext, encoding) = if serialized.len() > COMPRESS_THRESHOLD_BYTES {
        (gzip_bytes(&serialized)?, Some(ENCODING_GZIP.to_string()))
    } else {
        (serialized, None)
    };
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &plaintext,
                aad: to_device_id.as_bytes(),
            },
        )
        .map_err(|e| anyhow!("encrypt failed: {e}"))?;
    Ok(EncryptedEnvelope {
        to_device_id: to_device_id.to_string(),
        nonce: nonce_bytes.to_vec(),
        ciphertext,
        chunk_id: None,
        chunk_index: None,
        chunk_total: None,
        encoding,
    })
}

/// Decrypt an `EncryptedEnvelope` back into a typed `Envelope`. Verifies the
/// AEAD tag (rejecting tampering or wrong-key attempts) and checks
/// `proto_version` matches.
pub fn decrypt(key: &SessionKey, encrypted: &EncryptedEnvelope) -> Result<Envelope> {
    let cipher = ChaCha20Poly1305::new(key.as_key());
    if encrypted.nonce.len() != NONCE_LEN {
        return Err(anyhow!("invalid nonce length"));
    }
    let nonce = Nonce::from_slice(&encrypted.nonce);
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &encrypted.ciphertext,
                aad: encrypted.to_device_id.as_bytes(),
            },
        )
        .map_err(|e| anyhow!("decrypt failed: {e}"))?;
    let plaintext = match encrypted.encoding.as_deref() {
        Some(ENCODING_GZIP) => gunzip_bytes(&plaintext)?,
        Some(other) => return Err(anyhow!("unknown payload encoding: {other}")),
        None => plaintext,
    };
    let envelope: Envelope = serde_json::from_slice(&plaintext)?;
    if envelope.proto_version != PROTO_VERSION {
        return Err(anyhow!(
            "proto version mismatch: got {} expected {}",
            envelope.proto_version,
            PROTO_VERSION
        ));
    }
    Ok(envelope)
}

/// Gzip-compress `bytes` (single member, default compression level).
fn gzip_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 4), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|e| anyhow!("gzip failed: {e}"))?;
    encoder.finish().map_err(|e| anyhow!("gzip failed: {e}"))
}

/// Decompress a gzip payload. Accepts multi-member streams defensively.
fn gunzip_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len() * 4);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| anyhow!("gunzip failed: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_protocol::{ControlRequest, Message};
    use uuid::Uuid;

    #[test]
    fn encrypt_decrypt_roundtrip_recovers_envelope() {
        let key = SessionKey::derive(b"pairing-secret", b"kodex-relay-salt");
        let request_id = Uuid::new_v4();
        let envelope =
            Envelope::from_message(Some(request_id), &Message::ControlRequest(ControlRequest::Cancel { request_id }))
                .unwrap();
        let encrypted = encrypt(&key, "dev-phone", &envelope).unwrap();
        assert_eq!(encrypted.to_device_id, "dev-phone");
        assert_eq!(encrypted.nonce.len(), NONCE_LEN);
        assert!(!encrypted.ciphertext.is_empty());

        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, envelope);
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let key_a = SessionKey::derive(b"secret-a", b"salt");
        let key_b = SessionKey::derive(b"secret-b", b"salt");
        let envelope = Envelope::from_message(
            None,
            &Message::ControlRequest(ControlRequest::Cancel {
                request_id: Uuid::new_v4(),
            }),
        )
        .unwrap();
        let encrypted = encrypt(&key_a, "dev-phone", &envelope).unwrap();
        assert!(decrypt(&key_b, &encrypted).is_err());
    }

    #[test]
    fn to_device_id_mismatch_fails_aad_check() {
        let key = SessionKey::derive(b"secret", b"salt");
        let envelope = Envelope::from_message(
            None,
            &Message::ControlRequest(ControlRequest::Cancel {
                request_id: Uuid::new_v4(),
            }),
        )
        .unwrap();
        let encrypted = encrypt(&key, "dev-phone", &envelope).unwrap();
        let mut tampered = encrypted.clone();
        tampered.to_device_id = "dev-other".to_string();
        assert!(decrypt(&key, &tampered).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = SessionKey::derive(b"secret", b"salt");
        let envelope = Envelope::from_message(
            None,
            &Message::ControlRequest(ControlRequest::Cancel {
                request_id: Uuid::new_v4(),
            }),
        )
        .unwrap();
        let mut encrypted = encrypt(&key, "dev-phone", &envelope).unwrap();
        if let Some(byte) = encrypted.ciphertext.first_mut() {
            *byte ^= 0xff;
        }
        assert!(decrypt(&key, &encrypted).is_err());
    }

    #[test]
    fn small_payload_stays_uncompressed() {
        let key = SessionKey::derive(b"secret", b"salt");
        let envelope = Envelope::from_message(
            None,
            &Message::ControlRequest(ControlRequest::Cancel {
                request_id: Uuid::new_v4(),
            }),
        )
        .unwrap();
        let encrypted = encrypt(&key, "dev-phone", &envelope).unwrap();
        assert_eq!(encrypted.encoding, None);
        assert_eq!(decrypt(&key, &encrypted).unwrap(), envelope);
    }

    #[test]
    fn large_payload_is_gzipped_and_roundtrips() {
        let key = SessionKey::derive(b"secret", b"salt");
        // Repetitive JSON compresses well; must exceed COMPRESS_THRESHOLD_BYTES.
        // Raw Envelope (no typed message) keeps this test free of UiSnapshot
        // construction while matching the real snapshot_full shape.
        let envelope = Envelope {
            proto_version: PROTO_VERSION,
            id: None,
            message_type: "event".to_string(),
            payload: serde_json::json!({
                "kind": "snapshot_full",
                "snapshot": {
                    "filler": "x".repeat(64 * 1024),
                    "lines": (0..512)
                        .map(|i| format!("line-{i}-aaaaaaaaaaaaaaaaaaaa"))
                        .collect::<Vec<_>>(),
                },
            }),
        };
        let encrypted = encrypt(&key, "dev-phone", &envelope).unwrap();
        assert_eq!(encrypted.encoding.as_deref(), Some(ENCODING_GZIP));
        // Sanity: compression actually shrank this repetitive payload.
        let raw_len = serde_json::to_vec(&envelope).unwrap().len();
        assert!(
            encrypted.ciphertext.len() < raw_len / 2,
            "expected gzip to halve {raw_len} bytes, got {}",
            encrypted.ciphertext.len()
        );
        assert_eq!(decrypt(&key, &encrypted).unwrap(), envelope);
    }
}
