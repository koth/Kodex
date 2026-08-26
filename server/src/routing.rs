use relay_protocol::{EncryptedEnvelope, Envelope, Message, PeerSessionReset};

use crate::errors::{RelayError, Result};
use crate::state::AppState;

/// Upper bound for a single routed encrypted frame. Multi-megabyte frames
/// (e.g. an untrimmed UI snapshot) have been observed to kill the mobile
/// WebSocket before the phone can process them, so the relay refuses to
/// forward anything above this size. The PC side now trims remote snapshots;
/// this is a defensive backstop to avoid repeating the failure.
const MAX_ROUTED_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// Route an `EncryptedEnvelope` text frame from `from_device_id` to the
/// paired target. Only `to_device_id` is inspected (for routing and pairing
/// isolation); `ciphertext` and `nonce` are forwarded untouched.
pub async fn route_encrypted(
    state: &AppState,
    from_device_id: &str,
    text: &str,
) -> Result<()> {
    if text.len() > MAX_ROUTED_FRAME_BYTES {
        tracing::warn!(
            from = %from_device_id,
            bytes = text.len(),
            max = MAX_ROUTED_FRAME_BYTES,
            "dropping oversized encrypted frame (mobile WS safety limit)"
        );
        return Ok(());
    }
    let env: EncryptedEnvelope = serde_json::from_str(text)?;
    let paired = state
        .db
        .pairing_for(from_device_id.to_string(), env.to_device_id.clone())
        .await?;
    if paired.is_none() {
        return Err(RelayError::NotPaired);
    }
    match state.connections.get(&env.to_device_id) {
        Some(tx) => {
            if tx.send(text.to_string()).await.is_err() {
                // Dead connection entry (peer's WS dropped without cleanup):
                // evict so the device reconnects into a fresh slot instead
                // of frames being silently swallowed.
                tracing::warn!(
                    to_device_id = %env.to_device_id,
                    "encrypted frame delivery failed; evicting dead connection entry"
                );
                state.connections.remove(&env.to_device_id);
            } else {
                tracing::info!(
                    from = %from_device_id,
                    to = %env.to_device_id,
                    bytes = text.len(),
                    "encrypted frame routed"
                );
            }
        }
        None => {
            tracing::warn!(
                from = %from_device_id,
                to_device_id = %env.to_device_id,
                "target offline; dropping encrypted frame"
            );
        }
    }
    Ok(())
}

/// Route an advisory `peer_session_reset` from `from_device_id` to its latest
/// pairing partner. The notice is plaintext (it carries no secrets) and tells
/// the peer that the sender cannot decrypt the peer's traffic, so the peer
/// should drop its session key and re-run its resume handshake instead of
/// waiting out a control-request timeout.
pub async fn route_peer_session_reset(
    state: &AppState,
    from_device_id: &str,
) -> Result<()> {
    let Some(partner) = state
        .db
        .latest_pairing_partner_for(from_device_id.to_string())
        .await?
    else {
        tracing::debug!(
            from = %from_device_id,
            "peer_session_reset: sender has no pairing; ignoring"
        );
        return Ok(());
    };
    let envelope = Envelope::from_message(None, &Message::PeerSessionReset(PeerSessionReset {}))?;
    let text = serde_json::to_string(&envelope)?;
    match state.connections.get(&partner) {
        Some(tx) => {
            if tx.send(text).await.is_err() {
                tracing::warn!(
                    to_device_id = %partner,
                    "peer_session_reset delivery failed; evicting dead connection entry"
                );
                state.connections.remove(&partner);
            } else {
                tracing::info!(
                    from = %from_device_id,
                    to = %partner,
                    "peer_session_reset routed"
                );
            }
        }
        None => {
            tracing::debug!(
                from = %from_device_id,
                to = %partner,
                "peer_session_reset: target offline; dropping"
            );
        }
    }
    Ok(())
}
