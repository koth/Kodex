use relay_protocol::{
    Message, PairingConfirm, PairingInitiate, PairingRegister, PairingResume, SubscriptionStatus,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::errors::{RelayError, Result};
use crate::state::AppState;
use crate::wire::send_message;

/// PC -> relay: register a one-time pairing code bound to the sender's
/// connection so a scanning phone's `PairingInitiate` can be routed here.
/// Acknowledged with a `SubscriptionStatus` ack (the doc's relay->device
/// success-ack shape).
pub async fn handle_pairing_register(
    state: &AppState,
    req: PairingRegister,
    pc_device_id: &str,
    tx: &mpsc::Sender<String>,
) -> Result<()> {
    // §7: rate-limit pairing-code generation per PC device_id to deter
    // brute-force / flooding. Reuses the failure limiter as a request counter.
    if !state.rate_limiter.allowed(pc_device_id) {
        return Err(RelayError::Other("pairing code rate limited".into()));
    }
    state.rate_limiter.record_failure(pc_device_id);
    state
        .db
        .register_pairing_code(
            req.pairing_code.clone(),
            pc_device_id.to_string(),
            state.config.pairing_code_ttl_secs,
        )
        .await?;
    tracing::info!(
        pairing_code = %req.pairing_code,
        pc_device_id = %pc_device_id,
        "pairing code registered"
    );
    let ack = Message::SubscriptionStatus(SubscriptionStatus {
        active: false,
        plan: None,
        expires_at: None,
    });
    send_message(tx, None, &ack).await?;
    Ok(())
}

/// Phone -> relay: validate the pairing code, bind pc<->phone, mark the code
/// used, and send `PairingConfirm` to both peers. The relay forwards the
/// phone's ephemeral public key to the PC (via `session_key_material`) and
/// the PC's static public key to the phone; it never derives the E2E session
/// key.
pub async fn handle_pairing_initiate(
    state: &AppState,
    pi: PairingInitiate,
    phone_device_id: &str,
    tx: &mpsc::Sender<String>,
) -> Result<()> {
    let pc_device_id = state
        .db
        .take_pairing_code(pi.pairing_code.clone())
        .await?
        .ok_or(RelayError::InvalidPairingCode)?;
    let pairing_token = Uuid::new_v4().to_string();
    state
        .db
        .create_pairing(
            pairing_token.clone(),
            pc_device_id.clone(),
            phone_device_id.to_string(),
            pi.pc_device_pubkey.clone(),
        )
        .await?;
    state
        .db
        .mark_pairing_code_used(pi.pairing_code.clone())
        .await?;

    let phone_ephemeral = pi.phone_ephemeral_pubkey.clone().unwrap_or_default();
    let phone_confirm = Message::PairingConfirm(PairingConfirm {
        pairing_token: pairing_token.clone(),
        session_key_material: pi.pc_device_pubkey.clone(),
        pc_device_id: pc_device_id.clone(),
        phone_device_id: phone_device_id.to_string(),
    });
    let pc_confirm = Message::PairingConfirm(PairingConfirm {
        pairing_token: pairing_token.clone(),
        session_key_material: phone_ephemeral,
        pc_device_id: pc_device_id.clone(),
        phone_device_id: phone_device_id.to_string(),
    });

    // Send PC's confirm FIRST so it installs the session key before the
    // phone (which starts sending encrypted control requests immediately
    // after receiving its confirm). Reversing this order causes a race:
    // the phone's first encrypted frame can arrive at the PC before the
    // PC has installed the key, crashing the driver.
    if let Some(pc_tx) = state.connections.get(&pc_device_id) {
        send_message(&pc_tx, None, &pc_confirm).await?;
    } else {
        tracing::warn!(
            pc_device_id = %pc_device_id,
            "PC offline during pairing confirm; phone confirmed only (PC will not receive E2E material)"
        );
    }
    send_message(tx, None, &phone_confirm).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::connections::Connections;
    use crate::db::Db;
    use crate::ratelimit::RateLimiter;
    use crate::state::AppState;
    use relay_protocol::Envelope;

    fn app_state() -> AppState {
        AppState {
            config: Config::default(),
            db: Db::open_in_memory().unwrap(),
            connections: Connections::new(),
            rate_limiter: RateLimiter::new(10, 300),
        }
    }

    async fn seed_bound_pairing(state: &AppState, pc: &str, phone: &str, token: &str) {
        state.db.register_device(pc.to_string(), "pc-ed25519".into()).await.unwrap();
        state.db.register_device(phone.to_string(), "ph-ed25519".into()).await.unwrap();
        let token = token.to_string();
        state.db
            .create_pairing(token.to_string(), pc.to_string(), phone.to_string(), "pc-x25519".into())
            .await
            .unwrap();
        state.db.blocking(move |c| {
            c.execute(
                "INSERT INTO accounts (account_id, credentials, auth_token, email) \
                 VALUES ('acct', '{}', 'tok', 'a@example.com')",
                [],
            )?;
            c.execute(
                "UPDATE pairings SET bound = 1, account_id = 'acct' WHERE pairing_id = ?1",
                rusqlite::params![token.clone()],
            )?;
            c.execute(
                "INSERT INTO subscriptions (account_id, plan, active, expires_at) \
                 VALUES ('acct', 'monthly', 1, 9999999999999)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn resume_forwards_fresh_phone_ephemeral_to_online_pc() {
        let state = app_state();
        seed_bound_pairing(&state, "pc", "phone", "token").await;

        let (pc_tx, mut pc_rx) = mpsc::channel::<String>(8);
        state.connections.insert("pc", 1, pc_tx.clone());

        let (phone_tx, mut phone_rx) = mpsc::channel::<String>(8);
        handle_pairing_resume(
            &state,
            PairingResume {
                pairing_token: "token".into(),
                phone_ephemeral_pubkey: "eph".into(),
            },
            "phone",
            &phone_tx,
        )
        .await
        .unwrap();

        let pc_text = pc_rx.recv().await.unwrap();
        let pc_env: Envelope = serde_json::from_str(&pc_text).unwrap();
        match pc_env.into_message().unwrap() {
            Message::PairingConfirm(confirm) => {
                assert_eq!(confirm.pairing_token, "token");
                assert_eq!(confirm.session_key_material, "eph");
            }
            other => panic!("expected PairingConfirm to PC, got {other:?}"),
        }

        let phone_text = phone_rx.recv().await.unwrap();
        let phone_env: Envelope = serde_json::from_str(&phone_text).unwrap();
        match phone_env.into_message().unwrap() {
            Message::PairingConfirm(confirm) => {
                assert_eq!(confirm.session_key_material, "pc-x25519");
            }
            other => panic!("expected PairingConfirm to phone, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resume_works_without_active_subscription() {
        let state = app_state();
        seed_bound_pairing(&state, "pc", "phone", "token").await;
        state.db.deactivate_subscription("acct".into()).await.unwrap();

        let (pc_tx, _pc_rx) = mpsc::channel::<String>(8);
        state.connections.insert("pc", 1, pc_tx);
        let (phone_tx, mut phone_rx) = mpsc::channel::<String>(8);
        handle_pairing_resume(
            &state,
            PairingResume {
                pairing_token: "token".into(),
                phone_ephemeral_pubkey: "eph".into(),
            },
            "phone",
            &phone_tx,
        )
        .await
        .unwrap();

        let text = phone_rx.recv().await.unwrap();
        let env: Envelope = serde_json::from_str(&text).unwrap();
        assert!(matches!(
            env.into_message().unwrap(),
            Message::PairingConfirm(_)
        ));
    }
}

/// Phone/PC -> relay: resume an already-created pairing without re-scanning.
/// The phone mints a fresh ephemeral keypair and sends its public key; the
/// relay validates the persisted pairing token, then forwards the fresh
/// material to the paired PC as a `PairingConfirm`.
/// Both peers derive the same new E2E session key from their own secret.
pub async fn handle_pairing_resume(
    state: &AppState,
    req: PairingResume,
    phone_device_id: &str,
    tx: &mpsc::Sender<String>,
) -> Result<()> {
    let (pc_device_id, paired_phone, _account_id, pc_x25519_pubkey) = state
        .db
        .pairing_by_token(req.pairing_token.clone())
        .await?
        .ok_or(RelayError::InvalidPairingCode)?;
    if paired_phone != phone_device_id {
        return Err(RelayError::NotPaired);
    }

    let pairing_token = req.pairing_token;
    let pc_confirm = Message::PairingConfirm(PairingConfirm {
        pairing_token: pairing_token.clone(),
        session_key_material: req.phone_ephemeral_pubkey,
        pc_device_id: pc_device_id.clone(),
        phone_device_id: phone_device_id.to_string(),
    });

    let Some(pc_tx) = state.connections.get(&pc_device_id) else {
        return Err(RelayError::Other("paired PC is offline".into()));
    };
    send_message(&pc_tx, None, &pc_confirm).await?;

    // Acknowledge the phone with the same confirm so it can derive the
    // matching key; it already holds the PC static public key used for ECDH
    // from the bound-device record, so `session_key_material` is informational.
    let phone_confirm = Message::PairingConfirm(PairingConfirm {
        pairing_token,
        session_key_material: pc_x25519_pubkey,
        pc_device_id,
        phone_device_id: phone_device_id.to_string(),
    });
    send_message(tx, None, &phone_confirm).await?;
    Ok(())
}
