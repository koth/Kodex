use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use relay_protocol::{DeviceAuth, Message, SubscriptionStatus};
use tokio::sync::mpsc;

use crate::errors::{RelayError, Result};
use crate::state::AppState;
use crate::wire::send_message;

/// Validate that `device_id` is base64url-no-pad of a 32-byte (SHA-256-sized)
/// value. Matches the phone/relay-client encoding
/// (`base64::engine::general_purpose::URL_SAFE_NO_PAD`); using STANDARD here
/// rejected device ids containing `-`/`_`, breaking the auth handshake.
pub fn validate_device_id_format(device_id: &str) -> Result<()> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(device_id)
        .map_err(|_| RelayError::InvalidDeviceId(device_id.to_string()))?;
    if decoded.len() != 32 {
        return Err(RelayError::InvalidDeviceId(device_id.to_string()));
    }
    Ok(())
}

/// Validate `timestamp_ms` is within ±`window_secs` of now (replay window).
pub fn validate_timestamp(timestamp_ms: u64, window_secs: u64) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let ts = timestamp_ms as i64;
    let window_ms = (window_secs as i64) * 1000;
    if (ts - now).abs() > window_ms {
        return Err(RelayError::StaleTimestamp);
    }
    Ok(())
}

/// Authenticate a device connection (MVP).
///
/// Validates `device_id` format and `timestamp_ms` freshness, rate-limits
/// failures, and registers the device on first auth (requirements doc §6).
/// The HMAC `signature` is recorded for audit but not cryptographically
/// verified in the MVP (known contract gap; v2 upgrades to Ed25519). On
/// success, sends a `SubscriptionStatus` ack, which the PC accepts as the
/// auth ack alongside `DeviceAuth`.
pub async fn handle_device_auth(
    state: &AppState,
    auth: DeviceAuth,
    tx: &mpsc::Sender<String>,
) -> Result<()> {
    if !state.rate_limiter.allowed(&auth.device_id) {
        return Err(RelayError::Other("rate limited".to_string()));
    }
    let res = authenticate(state, &auth, tx).await;
    if res.is_err() {
        state.rate_limiter.record_failure(&auth.device_id);
    }
    res
}

/// Verify an Ed25519 `DeviceAuth` signature when `device_pubkey` is present.
/// The signed message is `{device_id}:{timestamp_ms}` (UTF-8), matching
/// `relay_client::identity::DeviceIdentity::auth_signature` and the phone's
/// `authSignature`. Returns `Ok(())` if the signature verifies or if
/// `device_pubkey` is absent (legacy peer — verification skipped).
pub fn verify_device_auth_signature(auth: &DeviceAuth) -> Result<()> {
    let pubkey_b64 = match &auth.device_pubkey {
        Some(pk) if !pk.is_empty() => pk,
        _ => return Ok(()), // legacy peer: no Ed25519 pubkey, skip verify
    };
    let pubkey_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(pubkey_b64)
        .map_err(|_| RelayError::InvalidDeviceId(pubkey_b64.clone()))?;
    let pubkey_arr: [u8; 32] = pubkey_bytes
        .as_slice()
        .try_into()
        .map_err(|_| RelayError::InvalidDeviceId(pubkey_b64.clone()))?;
    let verifying_key = VerifyingKey::from_bytes(&pubkey_arr)
        .map_err(|_| RelayError::InvalidDeviceId(pubkey_b64.clone()))?;
    let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&auth.signature)
        .map_err(|_| RelayError::InvalidSignature)?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|_| RelayError::InvalidSignature)?;
    let message = format!("{}:{}", auth.device_id, auth.timestamp_ms);
    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|_| RelayError::InvalidSignature)
}

async fn authenticate(
    state: &AppState,
    auth: &DeviceAuth,
    tx: &mpsc::Sender<String>,
) -> Result<()> {
    validate_device_id_format(&auth.device_id)?;
    validate_timestamp(auth.timestamp_ms, state.config.auth_timestamp_window_secs)?;
    // Verify the Ed25519 signature when a device_pubkey is supplied. Legacy
    // peers (no device_pubkey) skip verification for backward compat.
    verify_device_auth_signature(auth)?;
    // Store the Ed25519 public key so future cross-device routing / audit
    // can identify the signer; empty string for legacy peers.
    let public_key = auth.device_pubkey.clone().unwrap_or_default();
    state
        .db
        .register_device(auth.device_id.clone(), public_key)
        .await?;
    state.rate_limiter.reset(&auth.device_id);
    let verified = auth.device_pubkey.is_some();
    tracing::info!(
        device_id = %auth.device_id,
        signature_verified = verified,
        "device authenticated"
    );
    let ack = Message::SubscriptionStatus(SubscriptionStatus {
        active: false,
        plan: None,
        expires_at: None,
    });
    send_message(tx, None, &ack).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn device_id_format_accepts_32_byte_base64url() {
        let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        assert!(validate_device_id_format(&id).is_ok());
    }

    #[test]
    fn device_id_format_accepts_url_safe_chars() {
        // base64url uses `-` and `_`; ensure these decode (standard base64 would reject).
        let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xff; 32]);
        assert!(id.contains('-') || id.contains('_'));
        assert!(validate_device_id_format(&id).is_ok());
    }

    #[test]
    fn device_id_format_rejects_wrong_length() {
        let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 16]);
        assert!(validate_device_id_format(&id).is_err());
    }

    #[test]
    fn device_id_format_rejects_non_base64() {
        assert!(validate_device_id_format("not-base64url!!").is_err());
    }

    #[test]
    fn timestamp_window_rejects_skew() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(validate_timestamp(now_ms, 300).is_ok());
        assert!(validate_timestamp(now_ms + 1_000_000, 300).is_err());
        assert!(validate_timestamp(now_ms.saturating_sub(1_000_000), 300).is_err());
    }

    fn sign_auth(
        device_id: &str,
        timestamp_ms: u64,
    ) -> (String, String) {
        use ed25519_dalek::{Signer, SigningKey};
        use rand_core::OsRng;
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing_key.verifying_key().to_bytes());
        let message = format!("{}:{timestamp_ms}", device_id);
        let sig = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        (pubkey, sig_b64)
    }

    #[test]
    fn verify_accepts_valid_ed25519_signature() {
        let device_id =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        let ts = 1_700_000_000_000u64;
        let (pubkey, sig) = sign_auth(&device_id, ts);
        let auth = DeviceAuth {
            device_id,
            device_pubkey: Some(pubkey),
            signature: sig,
            timestamp_ms: ts,
        };
        assert!(verify_device_auth_signature(&auth).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let device_id =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        let ts = 1_700_000_000_000u64;
        let (pubkey, sig) = sign_auth(&device_id, ts);
        // Sign over a different device_id than the auth carries.
        let auth = DeviceAuth {
            device_id: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1u8; 32]),
            device_pubkey: Some(pubkey),
            signature: sig,
            timestamp_ms: ts,
        };
        assert!(matches!(
            verify_device_auth_signature(&auth),
            Err(RelayError::InvalidSignature)
        ));
    }

    #[test]
    fn verify_skips_when_no_pubkey_legacy() {
        let auth = DeviceAuth {
            device_id: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]),
            device_pubkey: None,
            signature: String::new(),
            timestamp_ms: 1_700_000_000_000,
        };
        assert!(verify_device_auth_signature(&auth).is_ok());
    }
}
