//! Account login (passwordless email-OTP) + account-session persistence.
//!
//! Complements [`binding`]: `BoundDevice` holds the `auth_token` *after* a
//! successful bind, while `AccountSession` holds the `auth_token` *acquired
//! by login* and feeds it into the subsequent `BindDeviceRequest`. Both are
//! persisted as JSON in the app data dir, separate from the device key and
//! from the E2E session key (which is re-derived per pairing).
//!
//! The wire protocol is untouched: `BindDeviceRequest { auth_token }` keeps
//! treating `auth_token` as an opaque string — only its minting source
//! changed from a placeholder to the relay's `/auth/*` HTTP endpoints.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Persisted account session acquired via the email-OTP login flow.
/// `auth_token` rotates on each login (server-side); `account_id` is
/// stable per email. None of these is the E2E session key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSession {
    pub email: String,
    pub account_id: String,
    pub auth_token: String,
}

impl AccountSession {
    /// Persist the session as JSON at `path`. Mirrors `BoundDevice::persist`.
    pub fn persist(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create account session dir {:?}", parent))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("write account session {:?}", path))?;
        Ok(())
    }

    /// Load a stored session, or `Ok(None)` if none exists.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let json =
            std::fs::read_to_string(path).with_context(|| format!("read account session {:?}", path))?;
        let session = serde_json::from_str(&json).context("parse account session")?;
        Ok(Some(session))
    }

    /// Delete the session (on explicit logout).
    pub fn clear(path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("remove account session {:?}", path))?;
        }
        Ok(())
    }
}

/// HTTP client for the relay's passwordless login endpoints
/// (`POST /auth/send-code`, `POST /auth/login`). `base_url` is the auth
/// HTTP origin (e.g. `https://relay.kodex.app` or `http://127.0.0.1:8789`);
/// the server serves `/auth/*` on a listener separate from the WebSocket.
pub struct LoginClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct LoginResponse {
    auth_token: String,
    account_id: String,
}

impl LoginClient {
    /// Build a client for the given auth HTTP origin. A trailing `/` is
    /// trimmed. The request timeout caps a stuck relay so the UI does not
    /// hang indefinitely on send-code/login.
    pub fn new(base_url: String) -> Self {
        let mut base = base_url;
        while base.ends_with('/') {
            base.pop();
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { base_url: base, http }
    }

    /// `POST /auth/send-code { email }`. Succeeds on 2xx; surfaces the
    /// server's `{ "error": "…" }` message on 4xx so the UI can show e.g.
    /// "请求过于频繁".
    pub async fn send_code(&self, email: &str) -> Result<()> {
        let url = format!("{}/auth/send-code", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "email": email }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("send-code request: {e}"))?;
        Self::ensure_ok(response).await
    }

    /// `POST /auth/login { email, code }`. On success returns the freshly
    /// issued `AccountSession` (the response carries `auth_token` +
    /// `account_id`; the email is the one the user typed).
    pub async fn login(&self, email: &str, code: &str) -> Result<AccountSession> {
        let url = format!("{}/auth/login", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "email": email, "code": code }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("login request: {e}"))?;
        if !response.status().is_success() {
            let message = Self::error_message(response).await;
            return Err(anyhow::anyhow!("{message}"));
        }
        let body: LoginResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("parse login response: {e}"))?;
        Ok(AccountSession {
            email: email.to_string(),
            account_id: body.account_id,
            auth_token: body.auth_token,
        })
    }

    async fn ensure_ok(response: reqwest::Response) -> Result<()> {
        if response.status().is_success() {
            return Ok(());
        }
        let message = Self::error_message(response).await;
        Err(anyhow::anyhow!("{message}"))
    }

    /// Extract a human-readable error from a non-2xx response. The relay
    /// returns `{ "error": "…" }`; fall back to `relay <status>: <body>`.
    async fn error_message(response: reqwest::Response) -> String {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                return error.to_string();
            }
        }
        format!("relay {status}: {text}")
    }
}

/// Derive the auth HTTP origin from a WebSocket relay endpoint:
/// `wss://host[:port][/path]` → `https://host[:port]`, `ws://` → `http://`.
/// Path/query/fragment are stripped (the relay serves `/auth/*` at the
/// origin root). Returns `None` for a non-`ws`/`wss` endpoint.
pub fn auth_base_url_from_ws_endpoint(ws_endpoint: &str) -> Option<String> {
    let (scheme, rest) = if let Some(rest) = ws_endpoint.strip_prefix("wss://") {
        ("https", rest)
    } else if let Some(rest) = ws_endpoint.strip_prefix("ws://") {
        ("http", rest)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn auth_base_url_maps_schemes_and_strips_path() {
        assert_eq!(
            auth_base_url_from_ws_endpoint("wss://relay.kodex.app").as_deref(),
            Some("https://relay.kodex.app")
        );
        assert_eq!(
            auth_base_url_from_ws_endpoint("ws://127.0.0.1:8787").as_deref(),
            Some("http://127.0.0.1:8787")
        );
        assert_eq!(
            auth_base_url_from_ws_endpoint("wss://relay.kodex.app/relay?token=x").as_deref(),
            Some("https://relay.kodex.app")
        );
        assert!(auth_base_url_from_ws_endpoint("https://relay.kodex.app").is_none());
        assert!(auth_base_url_from_ws_endpoint("relay.kodex.app").is_none());
        assert!(auth_base_url_from_ws_endpoint("wss:///").is_none());
    }

    #[test]
    fn account_session_persists_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.json");
        let session = AccountSession {
            email: "user@example.com".to_string(),
            account_id: "acc-1".to_string(),
            auth_token: "tok-1".to_string(),
        };
        session.persist(&path).unwrap();
        let loaded = AccountSession::load(&path).unwrap().unwrap();
        assert_eq!(loaded, session);
    }

    #[test]
    fn account_session_load_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(AccountSession::load(&path).unwrap().is_none());
    }

    #[test]
    fn account_session_clear_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.json");
        AccountSession {
            email: "u@e.com".to_string(),
            account_id: "a".to_string(),
            auth_token: "t".to_string(),
        }
        .persist(&path)
        .unwrap();
        AccountSession::clear(&path).unwrap();
        assert!(!path.exists());
        // clear is idempotent when absent
        AccountSession::clear(&path).unwrap();
    }

    /// Records the last received `(method, path, body)` and returns a canned
    /// response, so we can assert the client posted the right JSON *and*
    /// observe how it handles 200/400.
    #[derive(Clone)]
    struct CannedResponder {
        status: u16,
        body: String,
        last: Arc<std::sync::Mutex<Option<(String, String, String)>>>,
    }

    impl CannedResponder {
        fn new(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                last: Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn respond(&self, method: &str, path: &str, body: &str) -> (u16, String) {
            *self.last.lock().unwrap() =
                Some((method.to_string(), path.to_string(), body.to_string()));
            (self.status, self.body.clone())
        }

        fn last_request(&self) -> Option<(String, String, String)> {
            self.last.lock().unwrap().clone()
        }
    }

    async fn spawn_mock_auth_http(responder: CannedResponder) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let responder = responder.clone();
                tokio::spawn(async move {
                    let _ = handle_http(&mut stream, &responder).await;
                });
            }
        });
        url
    }

    async fn handle_http(
        stream: &mut tokio::net::TcpStream,
        responder: &CannedResponder,
    ) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(4096);
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(idx) = find_subsequence(&buf, b"\r\n\r\n") {
                break idx;
            }
            if buf.len() > 65_536 {
                return Ok(());
            }
        };
        let header_len = header_end + 4;
        let head = std::str::from_utf8(&buf[..header_len]).unwrap_or("");
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let content_length: usize = lines
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(0);
        let mut body = buf[header_len..].to_vec();
        while body.len() < content_length {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        let take = body.len().min(content_length);
        let body_str = String::from_utf8_lossy(&body[..take]).to_string();
        let (status, body_out) = responder.respond(&method, &path, &body_str);
        let reason = match status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            _ => "OK",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body_out.len(),
            body_out
        );
        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }

    fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[tokio::test]
    async fn send_code_posts_email_and_succeeds_on_200() {
        let responder = CannedResponder::new(200, r#"{"ok":true}"#);
        let url = spawn_mock_auth_http(responder.clone()).await;
        let client = LoginClient::new(url);
        client.send_code("user@example.com").await.expect("200 ok");

        let (method, path, body) = responder.last_request().expect("request recorded");
        assert_eq!(method, "POST");
        assert_eq!(path, "/auth/send-code");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["email"], "user@example.com");
    }

    #[tokio::test]
    async fn send_code_surfaces_error_message_on_400() {
        let responder = CannedResponder::new(400, r#"{"error":"请求过于频繁，请稍后再试"}"#);
        let url = spawn_mock_auth_http(responder.clone()).await;
        let client = LoginClient::new(url);
        let err = client
            .send_code("user@example.com")
            .await
            .expect_err("400 should error");
        assert!(err.to_string().contains("请求过于频繁"));
    }

    #[tokio::test]
    async fn login_parses_token_and_account_id() {
        let responder =
            CannedResponder::new(200, r#"{"auth_token":"tok-abc","account_id":"acc-7"}"#);
        let url = spawn_mock_auth_http(responder.clone()).await;
        let client = LoginClient::new(url);
        let session = client
            .login("user@example.com", "123456")
            .await
            .expect("200 ok");
        assert_eq!(session.email, "user@example.com");
        assert_eq!(session.account_id, "acc-7");
        assert_eq!(session.auth_token, "tok-abc");

        let (method, path, body) = responder.last_request().expect("request recorded");
        assert_eq!(method, "POST");
        assert_eq!(path, "/auth/login");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["email"], "user@example.com");
        assert_eq!(parsed["code"], "123456");
    }

    #[tokio::test]
    async fn login_surfaces_error_message_on_400() {
        let responder = CannedResponder::new(400, r#"{"error":"验证码错误"}"#);
        let url = spawn_mock_auth_http(responder.clone()).await;
        let client = LoginClient::new(url);
        let err = client
            .login("user@example.com", "000000")
            .await
            .expect_err("400 should error");
        assert!(err.to_string().contains("验证码错误"));
    }

    #[tokio::test]
    async fn login_client_trims_trailing_slash_in_base_url() {
        // Base URL with a trailing slash must still produce /auth/login
        // (not //auth/login).
        let responder =
            CannedResponder::new(200, r#"{"auth_token":"t","account_id":"a"}"#);
        let url = spawn_mock_auth_http(responder.clone()).await;
        let client = LoginClient::new(format!("{url}/"));
        client.login("u@e.com", "1").await.unwrap();
        let (_, path, _) = responder.last_request().unwrap();
        assert_eq!(path, "/auth/login");
    }
}
