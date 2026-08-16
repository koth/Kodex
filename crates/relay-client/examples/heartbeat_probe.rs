//! Minimal relay liveness probe: dial, authenticate with a throwaway device
//! identity, send plaintext heartbeats every 15s, log every inbound frame.
//! Run against the production relay to determine whether a well-behaved
//! plaintext-heartbeating client gets reaped by the 60s heartbeat timeout.
//!
//!   cargo run -p relay-client --example heartbeat_probe -- ws://host:port

use relay_client::{DeviceIdentity, dial_plain};
use relay_protocol::{Envelope, Message};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://120.48.49.190".to_string());
    println!("dialing {url}");
    let mut conn = dial_plain(&url, Duration::from_secs(30), false).await?;
    println!("connected");

    let identity = DeviceIdentity::generate();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let device_id = identity.device_id();
    let pubkey = identity.device_pubkey_b64();
    let sig = identity.auth_signature(ts);
    conn.authenticate(&device_id, Some(&pubkey), &sig, ts).await?;
    println!("authenticated as {device_id}");

    let heartbeat = serde_json::to_string(&Envelope::from_message(None, &Message::Heartbeat)?)?;
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    let start = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = interval.tick() => {
                conn.send_heartbeat(&heartbeat).await?;
                println!("{:>4?} heartbeat sent", start.elapsed());
            }
            frame = conn.recv_envelope() => {
                match frame {
                    Ok(Some(env)) => println!("{:>4?} recv {:?}", start.elapsed(), env.message_type),
                    Ok(None) => {
                        println!("{:>4?} CLOSED by relay", start.elapsed());
                        return Ok(());
                    }
                    Err(e) => {
                        println!("{:>4?} ERROR {e:#}", start.elapsed());
                        return Err(e);
                    }
                }
            }
        }
    }
}
