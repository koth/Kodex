//! Minimal HTTP surface for the passwordless login flow.
//!
//! Implemented over raw `tokio::net::TcpListener` like `health.rs` — no HTTP
//! framework dependency. Serves two JSON endpoints plus a CORS preflight:
//!
//! - `POST /auth/send-code`  `{ "email": "…" }`          → `{ "ok": true }`
//! - `POST /auth/login`      `{ "email","code": "…" }`   → `{ "auth_token", "account_id" }`
//! - `OPTIONS *`             → 204 (CORS preflight)
//!
//! Anything else returns 404. Logic errors (cooldown, wrong/expired code)
//! come back as 400 with `{ "error": "…" }`; transport/parse failures as 500.
//! The real work lives in `login.rs`; this module only frames HTTP.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::errors::{RelayError, Result};
use crate::login::{send_code, verify_and_login};
use crate::mail::{MailSender, ResendSender};
use crate::state::AppState;

/// Run the HTTP auth listener until the task is cancelled.
pub async fn run(state: AppState, addr: SocketAddr) -> Result<()> {
    // Build the production mail sender from config. The trait object lets
    // `serve` stay generic; tests would inject a mock, but the bare-TCP
    // adapter is thin enough that core logic is covered by `login.rs`.
    let mail: Arc<dyn MailSender> = Arc::new(ResendSender::new(
        state.config.resend_api_key.clone(),
        state.config.resend_from.clone(),
    ));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "auth http endpoint listening");
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                let mail = mail.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve(stream, state, mail.as_ref()).await {
                        tracing::debug!(%peer, error = %e, "auth http connection error");
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "auth http accept failed"),
        }
    }
}

async fn serve(
    mut stream: TcpStream,
    state: AppState,
    mail: &dyn MailSender,
) -> Result<()> {
    let req = read_request(&mut stream).await?;
    let (status, body) = route(&state, mail, req).await;
    write_response(&mut stream, status, &body).await?;
    Ok(())
}

struct Request {
    method: String,
    path: String,
    body: String,
}

async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .map_err(|_| RelayError::Other("read timeout".into()))?
            .map_err(RelayError::from)?;
        if n == 0 {
            return Err(RelayError::Other("connection closed before headers".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_subsequence(&buf, b"\r\n\r\n") {
            break idx;
        }
        if buf.len() > 65_536 {
            return Err(RelayError::Other("request too large".into()));
        }
    };
    let header_len = header_end + 4;
    let head = std::str::from_utf8(&buf[..header_len]).unwrap_or("");
    let request_line = head.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("").to_string();
    let path = raw_path
        .split('?')
        .next()
        .unwrap_or(&raw_path)
        .to_string();
    let content_length = head
        .lines()
        .find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);

    let mut body = buf[header_len..].to_vec();
    while body.len() < content_length {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .map_err(|_| RelayError::Other("read timeout".into()))?
            .map_err(RelayError::from)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    let take = body.len().min(content_length);
    let body = String::from_utf8_lossy(&body[..take]).to_string();
    Ok(Request { method, path, body })
}

async fn route(state: &AppState, mail: &dyn MailSender, req: Request) -> (u16, String) {
    if req.method == "OPTIONS" {
        return (204, String::new());
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/auth/send-code") => {
            let email = match parse_field::<SendCodeReq>(&req.body).map(|r| r.email) {
                Ok(e) => e,
                Err(m) => return json_err(400, &m),
            };
            match send_code(&state.db, mail, &email).await {
                Ok(()) => (200, serde_json::json!({"ok": true}).to_string()),
                Err(e) => json_err(400, &e.to_string()),
            }
        }
        ("POST", "/auth/login") => {
            let parsed = parse_field::<LoginReq>(&req.body);
            let (email, code) = match parsed.map(|r| (r.email, r.code)) {
                Ok(v) => v,
                Err(m) => return json_err(400, &m),
            };
            match verify_and_login(&state.db, &email, &code).await {
                Ok((account_id, token)) => (
                    200,
                    serde_json::json!({"auth_token": token, "account_id": account_id})
                        .to_string(),
                ),
                Err(e) => json_err(400, &e.to_string()),
            }
        }
        _ => (
            404,
            serde_json::json!({"error": "not found"}).to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct SendCodeReq {
    email: String,
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    code: String,
}

fn parse_field<'a, T: serde::Deserialize<'a>>(body: &'a str) -> std::result::Result<T, String> {
    serde_json::from_str(body).map_err(|e| format!("invalid request body: {e}"))
}

fn json_err(status: u16, message: &str) -> (u16, String) {
    (status, serde_json::json!({"error": message}).to_string())
}

async fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    response.push_str("Content-Type: application/json\r\n");
    response.push_str("Access-Control-Allow-Origin: *\r\n");
    response.push_str("Access-Control-Allow-Headers: Content-Type\r\n");
    response.push_str("Access-Control-Allow-Methods: POST, OPTIONS\r\n");
    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Connection: close\r\n\r\n");
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    Ok(())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
