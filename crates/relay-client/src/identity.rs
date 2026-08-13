//! Device identity for relay authentication.
//!
//! Each kodex instance holds two independent keypairs:
//! - **X25519** static keypair — the E2E key-exchange key (pairing ECDH).
//!   Never used for signing.
//! - **Ed25519** signing keypair — authenticates to the relay. The device
//!   signs `{device_id}:{timestamp_ms}` with the Ed25519 secret; the relay
//!   verifies with the Ed25519 public key carried in `DeviceAuth`.
//!
//! The device id is `base64url-no-pad(SHA-256(x25519_public_key))` — it
//! identifies the device and is stable across restarts. The Ed25519 public
//! key is sent alongside each `DeviceAuth` so the relay can verify the
//! signature without previously storing it.
//!
//! Persistence: a 64-byte file — the 32-byte X25519 secret followed by the
//! 32-byte Ed25519 secret. Legacy 32-byte files (X25519-only) are migrated
//! by generating a fresh Ed25519 keypair on load.

use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};

/// Secret file layout: `[x25519_secret (32)] [ed25519_secret (32)]` = 64 bytes.
const SECRET_FILE_LEN: usize = 64;

/// A device keypair + derived identity material.
#[derive(Clone)]
pub struct DeviceIdentity {
    /// X25519 static keypair (E2E key exchange only).
    secret: StaticSecret,
    public: PublicKey,
    /// Ed25519 signing keypair (relay authentication).
    signing_key: SigningKey,
}

impl DeviceIdentity {
    /// Generate a fresh identity using the OS RNG.
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(&mut OsRng);
        let public = PublicKey::from(&secret);
        let signing_key = SigningKey::generate(&mut OsRng);
        Self {
            secret,
            public,
            signing_key,
        }
    }

    /// Reconstruct from stored secret bytes. Accepts 64 bytes
    /// (X25519 || Ed25519) or a legacy 32-byte (X25519-only) file, generating
    /// a fresh Ed25519 keypair for the latter.
    pub fn from_bytes(secret_bytes: &[u8]) -> Self {
        if secret_bytes.len() == SECRET_FILE_LEN {
            let mut x = [0u8; 32];
            x.copy_from_slice(&secret_bytes[..32]);
            let mut e = [0u8; 32];
            e.copy_from_slice(&secret_bytes[32..]);
            let secret = StaticSecret::from(x);
            let public = PublicKey::from(&secret);
            let signing_key = SigningKey::from_bytes(&e);
            Self {
                secret,
                public,
                signing_key,
            }
        } else {
            // Legacy 32-byte X25519-only file, or any other length: generate
            // fresh X25519 + Ed25519 material (callers of from_bytes with a
            // 32-byte slice get a migrated identity with a new signing key).
            let mut x = [0u8; 32];
            if secret_bytes.len() == 32 {
                x.copy_from_slice(secret_bytes);
            }
            let secret = StaticSecret::from(x);
            let public = PublicKey::from(&secret);
            let signing_key = SigningKey::generate(&mut OsRng);
            Self {
                secret,
                public,
                signing_key,
            }
        }
    }

    /// Raw 32-byte X25519 secret (for persistence / E2E).
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// Raw 32-byte Ed25519 signing secret (for persistence).
    pub fn signing_secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// 64-byte secret blob for persistence: X25519 secret || Ed25519 secret.
    pub fn persist_bytes(&self) -> [u8; SECRET_FILE_LEN] {
        let mut out = [0u8; SECRET_FILE_LEN];
        out[..32].copy_from_slice(&self.secret.to_bytes());
        out[32..].copy_from_slice(&self.signing_key.to_bytes());
        out
    }

    /// X25519 public key bytes (for the QR payload / relay registration).
    pub fn public_bytes(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    /// Ed25519 verifying (public) key bytes, base64url-no-pad. Sent in
    /// `DeviceAuth` so the relay can verify `auth_signature`.
    pub fn device_pubkey_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.signing_key.verifying_key().to_bytes())
    }

    /// Stable device identifier: base64url-no-pad(SHA-256(x25519_public_key)).
    pub fn device_id(&self) -> String {
        let hash = Sha256::digest(self.public.to_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
    }

    /// X25519 public key, base64url-no-pad, for the QR pairing payload.
    pub fn public_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.public.to_bytes())
    }

    /// Derive the E2E session key from the PC static X25519 secret and the
    /// phone's ephemeral public key (base64url-no-pad), matching the phone's
    /// `ecdhSharedSecret` + `deriveSessionKey`. Salt defaults to the same
    /// `kodex-relay-salt` the phone uses.
    pub fn derive_pairing_session_key(
        &self,
        phone_ephemeral_pubkey_b64: &str,
        salt: &[u8],
    ) -> Result<crate::crypto::SessionKey> {
        let pub_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(phone_ephemeral_pubkey_b64)?;
        let arr: [u8; 32] = pub_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("phone ephemeral pubkey must be 32 bytes"))?;
        let phone_pub = PublicKey::from(arr);
        let shared = self.secret.diffie_hellman(&phone_pub).to_bytes();
        Ok(crate::crypto::SessionKey::derive(&shared, salt))
    }

    /// Ed25519 signature over `{device_id}:{timestamp_ms}`, base64url-no-pad.
    /// The relay verifies this with `device_pubkey_b64`.
    pub fn auth_signature(&self, timestamp_ms: u64) -> String {
        let message = format!("{}:{timestamp_ms}", self.device_id());
        let sig: Signature = self.signing_key.sign(message.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes())
    }

    /// Persist the secret to `path` (64 raw bytes). Created if missing.
    pub fn persist(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create device-key dir {:?}", parent))?;
        }
        std::fs::write(path, self.persist_bytes())
            .with_context(|| format!("write device key {:?}", path))?;
        Ok(())
    }

    /// Load the identity from `path`, generating + persisting a fresh one if
    /// the file does not exist. Legacy 32-byte files are migrated to 64 bytes.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path).with_context(|| format!("read device key {:?}", path))?;
            if bytes.len() != SECRET_FILE_LEN && bytes.len() != 32 {
                anyhow::bail!("device key file is {} bytes, expected 32 or 64", bytes.len());
            }
            let identity = Self::from_bytes(&bytes);
            // Re-persist if we migrated a legacy 32-byte file (or otherwise
            // expanded the layout) so future loads read 64 bytes directly.
            if bytes.len() != SECRET_FILE_LEN {
                identity.persist(path)?;
            }
            return Ok(identity);
        }
        let identity = Self::generate();
        identity.persist(path)?;
        Ok(identity)
    }

    /// Verify an Ed25519 `auth_signature` against a `device_pubkey` (both
    /// base64url-no-pad) over `{device_id}:{timestamp_ms}`. Used by the relay
    /// and by tests.
    pub fn verify_auth_signature(
        device_pubkey_b64: &str,
        device_id: &str,
        timestamp_ms: u64,
        signature_b64: &str,
    ) -> Result<()> {
        let pubkey_bytes =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(device_pubkey_b64)?;
        let sig_bytes =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature_b64)?;
        let verifying_key = VerifyingKey::from_bytes(
            pubkey_bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("ed25519 pubkey must be 32 bytes"))?,
        )?;
        let signature = Signature::from_slice(&sig_bytes)?;
        let message = format!("{}:{timestamp_ms}", device_id);
        verifying_key
            .verify(message.as_bytes(), &signature)
            .map_err(|_| anyhow::anyhow!("invalid device auth signature"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_stable_for_same_secret() {
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::from_bytes(&a.persist_bytes());
        assert_eq!(a.device_id(), b.device_id());
        assert_eq!(a.public_bytes(), b.public_bytes());
        assert_eq!(a.device_pubkey_b64(), b.device_pubkey_b64());
    }

    #[test]
    fn different_identities_have_different_ids() {
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        assert_ne!(a.device_id(), b.device_id());
    }

    #[test]
    fn auth_signature_is_deterministic_for_same_timestamp() {
        let id = DeviceIdentity::generate();
        let sig_a = id.auth_signature(1_700_000_000_000);
        let sig_b = id.auth_signature(1_700_000_000_000);
        assert_eq!(sig_a, sig_b);
        let sig_other_ts = id.auth_signature(1_700_000_000_001);
        assert_ne!(sig_a, sig_other_ts);
    }

    #[test]
    fn auth_signature_verifies_with_device_pubkey() {
        let id = DeviceIdentity::generate();
        let ts = 1_700_000_000_000u64;
        let sig = id.auth_signature(ts);
        DeviceIdentity::verify_auth_signature(&id.device_pubkey_b64(), &id.device_id(), ts, &sig)
            .expect("signature verifies");
    }

    #[test]
    fn auth_signature_rejects_wrong_device_id() {
        let id = DeviceIdentity::generate();
        let ts = 1_700_000_000_000u64;
        let sig = id.auth_signature(ts);
        assert!(DeviceIdentity::verify_auth_signature(
            &id.device_pubkey_b64(),
            "other-device-id",
            ts,
            &sig
        )
        .is_err());
    }

    #[test]
    fn persist_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        let id = DeviceIdentity::load_or_create(&path).unwrap();
        let original_id = id.device_id();
        let original_pk = id.device_pubkey_b64();
        let reloaded = DeviceIdentity::load_or_create(&path).unwrap();
        assert_eq!(reloaded.device_id(), original_id);
        assert_eq!(reloaded.device_pubkey_b64(), original_pk);
    }

    #[test]
    fn legacy_32_byte_file_migrates_to_ed25519() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        // Write a legacy 32-byte X25519-only file.
        let legacy = DeviceIdentity::generate();
        std::fs::write(&path, legacy.secret_bytes()).unwrap();
        let reloaded = DeviceIdentity::load_or_create(&path).unwrap();
        // X25519 device id is preserved; an Ed25519 signing key is added.
        assert_eq!(reloaded.device_id(), legacy.device_id());
        assert!(!reloaded.device_pubkey_b64().is_empty());
        // The file is re-written as 64 bytes.
        assert_eq!(std::fs::read(&path).unwrap().len(), SECRET_FILE_LEN);
    }
}
