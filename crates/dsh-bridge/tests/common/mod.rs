//! A fake dsh web host for integration tests.
//!
//! Speaks just enough of the harness RPC surface to exercise the bridge:
//! `POST /api/<method>` control responses and the `GET /api/events.mux` /
//! `GET /api/events.host` SSE streams. Frame delivery is scripted by the
//! test: the mux stream replays a configurable list of frames (each a
//! `ServerRequest` JSON), can drop the connection mid-way to exercise the
//! bridge's reconnection, and can fail a `session.history` call to exercise
//! per-session isolation.

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::ReadBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Wraps a `TcpStream` with a prefix of already-read bytes, so a WebSocket
/// handshake can re-read the HTTP upgrade request that `handle_connection`
/// already consumed from the socket.
struct PrefixedStream {
    prefix: Vec<u8>,
    prefix_pos: usize,
    inner: TcpStream,
}

impl tokio::io::AsyncRead for PrefixedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        if this.prefix_pos < this.prefix.len() {
            let remaining = &this.prefix[this.prefix_pos..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            this.prefix_pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(_cx, buf)
    }
}

impl tokio::io::AsyncWrite for PrefixedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = unsafe { self.get_unchecked_mut() };
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = unsafe { self.get_unchecked_mut() };
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// What the mux stream does when the test script ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxEnd {
    /// Hold the connection open after the scripted frames (idle keep-alive).
    Hold,
    /// Close the connection after the scripted frames (triggers reconnect).
    Close,
}

/// Behavior for a single mux stream connection (a reconnection re-runs the
/// script from its own frame list).
#[derive(Debug, Clone)]
pub struct MuxScript {
    /// Frames to emit as `data: <json>\n\n` on this connection.
    pub frames: Vec<Value>,
    /// What to do after `frames` are emitted.
    pub end: MuxEnd,
    /// Wait for the client's mux subscribe message before emitting frames, so
    /// a scripted server-request never races past the bridge's session
    /// registration (which precedes the subscribe).
    pub wait_for_subscribe: bool,
}

#[derive(Debug, Clone)]
pub struct MockHarnessConfig {
    /// Script for the first mux connection. Reconnections use
    /// `reconnect_scripts` when non-empty, else `mux` again.
    pub mux: Vec<MuxScript>,
    /// Per-session history failure: session ids whose `session.history` call
    /// should return an error (to exercise per-session re-baseline isolation).
    pub history_failures: Vec<String>,
    /// Events returned by `session.history` (each a `HistoryEntry` JSON
    /// `{ event: { type, seq, time, data }, view? }`).
    pub history_events: Vec<Value>,
}

#[derive(Debug, Default)]
struct MockState {
    /// Methods received (POST /api/<method>) with their rpcId, in order.
    pub calls: Vec<(String, String)>,
    /// `session.create` payloads received, in order.
    pub creates: Vec<Value>,
    /// `respond` payloads received (approval/question answers).
    pub responds: Vec<Value>,
}

pub struct MockHarness {
    pub addr: SocketAddr,
    config: Arc<Mutex<MockHarnessConfig>>,
    state: Arc<Mutex<MockState>>,
    /// Notifies a waiting test that the mux stream reached the end of its
    /// scripted frames (used to observe a drop).
    mux_dropped: Mutex<Option<mpsc::Receiver<()>>>,
    /// Tracks how many mux connections have been served.
    mux_conns: Arc<std::sync::atomic::AtomicUsize>,
}

impl MockHarness {
    /// Start the mock host on a random loopback port.
    pub async fn start(config: MockHarnessConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Arc::new(Mutex::new(config));
        let state = Arc::new(Mutex::new(MockState::default()));
        let (drop_tx, drop_rx) = mpsc::channel(1);
        let mux_conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let config_handle = config.clone();
        let state_handle = state.clone();
        let mux_conns_handle = mux_conns.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let config = config_handle.clone();
                let state = state_handle.clone();
                let drop_tx = drop_tx.clone();
                let mux_conns = mux_conns_handle.clone();
                tokio::spawn(async move {
                    handle_connection(stream, config, state, drop_tx, mux_conns).await;
                });
            }
        });

        Self {
            addr,
            config,
            state,
            mux_dropped: Mutex::new(Some(drop_rx)),
            mux_conns,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn calls(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().calls.clone()
    }

    /// Payloads of the `session.create` calls received, in order.
    pub fn creates(&self) -> Vec<Value> {
        self.state.lock().unwrap().creates.clone()
    }

    pub fn responds(&self) -> Vec<Value> {
        self.state.lock().unwrap().responds.clone()
    }

    pub fn mux_connection_count(&self) -> usize {
        self.mux_conns.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wait until the mux stream has emitted its scripted frames (and, for a
    /// `Close` script, dropped). Bounded by a timeout.
    pub async fn wait_for_mux_drop(&self) {
        let mut guard = self.mux_dropped.lock().unwrap();
        if let Some(rx) = guard.as_mut() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        }
    }

    /// Replace the connection script (e.g. a reconnection script with a
    /// different frame set).
    pub fn set_config(&self, config: MockHarnessConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Append scripts for subsequent mux connections.
    pub fn append_mux_scripts(&self, scripts: Vec<MuxScript>) {
        let mut guard = self.config.lock().unwrap();
        guard.mux.extend(scripts);
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    config: Arc<Mutex<MockHarnessConfig>>,
    state: Arc<Mutex<MockState>>,
    drop_tx: mpsc::Sender<()>,
    mux_conns: Arc<std::sync::atomic::AtomicUsize>,
) {
    let mut buf = Vec::new();
    // Read the request head (until \r\n\r\n).
    let mut tmp = [0u8; 4096];
    let head_end;
    loop {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_subslice(&buf, b"\r\n\r\n") {
            head_end = idx + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // Content-Length body (control POSTs).
    let content_length = lines
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + content_length {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = if buf.len() >= head_end + content_length {
        buf[head_end..head_end + content_length].to_vec()
    } else {
        Vec::new()
    };

    match (method.as_str(), path.as_str()) {
        ("GET", "/api/events.mux") => {
            mux_conns.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let scripts = {
                let mut guard = config.lock().unwrap();
                // Take the next script; keep the remainder for reconnections.
                let next = if guard.mux.is_empty() {
                    None
                } else {
                    guard.mux.drain(..1).next()
                };
                if next.is_none() {
                    // No scripts configured or none remaining: use the
                    // steady-state idle keep-alive.
                    guard.mux = scripts_for_hold();
                }
                next.unwrap_or_else(|| scripts_for_hold().remove(0))
            };
            serve_mux(stream, buf[..head_end].to_vec(), scripts, drop_tx).await;
        }
        ("GET", "/api/events.host") => {
            // Host stream: WebSocket keep-alive with no frames (idle).
            let prefixed = PrefixedStream {
                prefix: buf[..head_end].to_vec(),
                prefix_pos: 0,
                inner: stream,
            };
            if let Ok(mut ws) = tokio_tungstenite::accept_async(prefixed).await {
                while ws.next().await.is_some() {}
            }
        }
        ("POST", "/api/respond") => {
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            state.lock().unwrap().responds.push(parsed.clone());
            let receipt = serde_json::json!({ "accepted": true });
            let body = serde_json::to_vec(&receipt).unwrap();
            write_response(&mut stream, 200, "application/json", &body).await;
        }
        ("POST", path) => {
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let rpc_id = parsed
                .get("rpcId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let method_name = path.strip_prefix("/api/").unwrap_or("").to_string();
            let session_id = parsed
                .pointer("/payload/sessionId")
                .and_then(Value::as_str)
                .unwrap_or("s-mock")
                .to_string();
            state
                .lock()
                .unwrap()
                .calls
                .push((method_name.clone(), rpc_id.clone()));
            if method_name == "session.create" {
                let payload = parsed.get("payload").cloned().unwrap_or(Value::Null);
                state.lock().unwrap().creates.push(payload);
            }

            let value = match method_name.as_str() {
                "host.describe" => serde_json::json!({
                    "version": "0.1.0-test",
                    "cwd": "/tmp",
                    "attachedSessions": 0,
                    "canOpenPath": false,
                }),
                "session.list" => serde_json::json!({ "items": [] }),
                "session.create" => serde_json::json!({ "sessionId": "s-1" }),
                "session.prompt" => serde_json::json!({ "accepted": true }),
                "session.cancel" => serde_json::json!({ "accepted": true }),
                "session.history" => {
                    let fail = config
                        .lock()
                        .unwrap()
                        .history_failures
                        .iter()
                        .any(|id| *id == session_id);
                    if fail {
                        let response = serde_json::json!({
                            "type": "server-response",
                            "rpcId": rpc_id,
                            "result": {
                                "ok": false,
                                "error": { "code": "internal", "message": "history failure", "details": {} }
                            }
                        });
                        let body = serde_json::to_vec(&response).unwrap();
                        write_response(&mut stream, 200, "application/json", &body).await;
                        return;
                    }
                    let events = config.lock().unwrap().history_events.clone();
                    serde_json::json!({ "events": events, "hasMore": false })
                }
                "session.models" => serde_json::json!({
                    "current": { "provider": "deepseek", "model": "deepseek-v4-pro" },
                    "routable": true,
                    "groups": [
                        {
                            "id": "deepseek",
                            "name": "DeepSeek",
                            "models": [
                                { "id": "deepseek-v4-pro", "name": "DeepSeek V4 Pro" },
                                { "id": "deepseek-v4-flash", "name": "DeepSeek V4 Flash" }
                            ]
                        }
                    ],
                    "failures": [],
                }),
                "session.selectModel" => serde_json::json!({
                    "selected": { "provider": "deepseek", "model": "deepseek-v4-pro" }
                }),
                _ => serde_json::json!({}),
            };
            let response = serde_json::json!({
                "type": "server-response",
                "rpcId": rpc_id,
                "result": { "ok": true, "value": value }
            });
            let body = serde_json::to_vec(&response).unwrap();
            write_response(&mut stream, 200, "application/json", &body).await;
        }
        ("GET", _) => {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
        _ => {
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
    }
}

async fn serve_mux(stream: TcpStream, head: Vec<u8>, script: MuxScript, drop_tx: mpsc::Sender<()>) {
    let prefixed = PrefixedStream {
        prefix: head,
        prefix_pos: 0,
        inner: stream,
    };
    let mut ws = match tokio_tungstenite::accept_async(prefixed).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    for frame in &script.frames {
        let payload = serde_json::to_string(frame).unwrap();
        let _ = ws.send(Message::Text(payload.into())).await;
    }
    match script.end {
        MuxEnd::Hold => {
            // Idle keep-alive until the client disconnects. Drain incoming
            // (client→host is a protocol violation; tungstenite closes on it).
            while ws.next().await.is_some() {}
        }
        MuxEnd::Close => {
            let _ = drop_tx.send(()).await;
            let _ = ws.close(None).await;
        }
    }
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn scripts_for_hold() -> Vec<MuxScript> {
    vec![MuxScript {
        frames: Vec::new(),
        end: MuxEnd::Hold,
        wait_for_subscribe: true,
    }]
}

/// Build a mux `session/event` `ServerRequest` frame.
pub fn mux_session_event(session_id: &str, seq: u64, type_tag: &str, data: Value) -> Value {
    serde_json::json!({
        "type": "server-request",
        "rpcId": format!("rpc-{session_id}-{seq}"),
        "method": "session/event",
        "payload": {
            "type": "session/event",
            "sessionId": session_id,
            "event": { "type": type_tag, "seq": seq, "time": 0.0, "data": data }
        }
    })
}

/// Build a mux `session/subscribed` `ServerRequest` frame.
pub fn mux_subscribed(session_id: &str, last_seq: i64) -> Value {
    serde_json::json!({
        "type": "server-request",
        "rpcId": format!("rpc-sub-{session_id}"),
        "method": "session/event",
        "payload": {
            "type": "session/subscribed",
            "sessionId": session_id,
            "lastSeq": last_seq
        }
    })
}

/// A single-connection script list (one mux connection that holds open).
pub fn scripts_with(frames: Vec<Value>) -> Vec<MuxScript> {
    vec![MuxScript {
        frames,
        end: MuxEnd::Hold,
        wait_for_subscribe: true,
    }]
}

pub fn default_config() -> MockHarnessConfig {
    MockHarnessConfig {
        mux: Vec::new(),
        history_failures: Vec::new(),
        history_events: Vec::new(),
    }
}

/// A `SessionEvent` JSON for `session.history` replay.
pub fn history_event(seq: u64, type_tag: &str, data: Value) -> Value {
    serde_json::json!({
        "event": {
            "type": type_tag,
            "seq": seq,
            "time": 0.0,
            "data": data
        }
    })
}
