// Smoke test: hit the relay's passwordless login endpoint with TLS
// verification disabled (self-signed host). Confirms the auth HTTP surface
// is reachable through the reverse proxy. Does NOT submit a real code.
//
// Usage: cargo run -p relay-client --example login_insecure -- https://host

use relay_client::LoginClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://127.0.0.1".to_string());
    let client = LoginClient::new(base.clone(), true);
    println!("POST {base}/auth/send-code (insecure TLS) with dummy email...");
    match client.send_code("smoke@example.invalid").await {
        Ok(()) => println!("OK: send-code returned 2xx (server accepted the request)"),
        Err(e) => {
            // A 4xx (e.g. cooldown / invalid email) still proves the HTTP
            // path is reachable; only transport failures mean it's broken.
            let msg = format!("{e:#}");
            if msg.contains("request") || msg.contains("dns") || msg.contains("tls") || msg.contains("connect") {
                eprintln!("TRANSPORT ERR: {msg}");
                std::process::exit(1);
            }
            println!("REACHABLE (non-2xx, expected for dummy email): {msg}");
        }
    }
    Ok(())
}
