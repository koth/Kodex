use relay_protocol::EncryptedEnvelope;

use crate::errors::{RelayError, Result};
use crate::state::AppState;

/// Route an `EncryptedEnvelope` text frame from `from_device_id` to the
/// paired target. Only `to_device_id` is inspected (for routing and pairing
/// isolation); `ciphertext` and `nonce` are forwarded untouched.
pub async fn route_encrypted(
    state: &AppState,
    from_device_id: &str,
    text: &str,
) -> Result<()> {
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
