// Smoke test: dial a `wss://` relay endpoint with TLS verification disabled
// (self-signed host). Confirms the WebSocket handshake completes against a
// deployed relay. Intended for manual verification during development.
//
// Usage: cargo run -p relay-client --example dial_insecure -- wss://host:port

use relay_client::dial_plain;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "wss://127.0.0.1:8787".to_string());
    println!("dialing {url} (insecure TLS)...");
    match dial_plain(&url, Duration::from_secs(30), true).await {
        Ok(_) => println!("OK: relay WebSocket handshake succeeded"),
        Err(e) => {
            eprintln!("ERR: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}
