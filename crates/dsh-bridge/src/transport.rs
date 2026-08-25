//! HTTP + WebSocket transport for the dsh host RPC.
//!
//! Control plane: `POST /api/<method>` with a [`ClientRequest`] body, parsing
//! the [`ServerResponse`] and verifying the echoed `rpcId` (mirrors
//! `AbstractApiClient.callUnary` in
//! `deepseek-harness/packages/host/apiproxy/src/fetch/client.ts`).
//!
//! Event plane: `GET /api/events.mux` and `GET /api/events.host` upgraded to
//! WebSocket (dsh's `client-connection` plugin answers these GETs with 426
//! Upgrade Required and serves frames over WebSocket text messages). Each WS
//! text message is a JSON [`ServerRequest`]; the payload is narrowed to the
//! frame union `F` by the caller. A malformed frame is logged and skipped —
//! one corrupt frame must not kill the stream.

use anyhow::{Context, anyhow};
use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;

use crate::rpc_types::{
    ClientRequest, ClientResponse, HostDescribePayload, HostDescribeValue, RpcId, RpcReceipt,
    ServerRequest, ServerResponse, SessionCancelPayload, SessionCancelValue, SessionCreatePayload,
    SessionCreateValue, SessionHistoryPayload, SessionHistoryValue, SessionListPayload,
    SessionListValue, SessionModelsPayload, SessionPromptPayload, SessionPromptValue,
    SessionSelectModelPayload,
};

/// Default timeout for bounded control calls (a hung host must not leave the
/// session pending forever). Matches dsh's `DEFAULT_TIMEOUT_MS`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared HTTP client for a harness host. Connection pooling multiplexes
/// concurrent control POSTs from multiple sessions; the cookie jar is empty for
/// loopback. Cloning is cheap (Arc internals).
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    base_url: reqwest::Url,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("base_url", &self.base_url.as_str())
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    pub fn new(endpoint: &str) -> anyhow::Result<Self> {
        let base_url = reqwest::Url::parse(endpoint.trim_end_matches('/'))
            .with_context(|| format!("invalid harness endpoint: {endpoint}"))?;
        let inner = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("failed to build reqwest client")?;
        Ok(Self { inner, base_url })
    }

    pub fn endpoint(&self) -> &str {
        self.base_url.as_str()
    }

    fn api_url(&self, path: &str) -> reqwest::Url {
        // dsh serves every RPC endpoint under `/api/`:
        //   POST /api/<method>        (e.g. session.create, host.describe)
        //   POST /api/respond
        //   GET  /api/events.mux | /api/events.host
        self.base_url
            .join(&format!("/api/{path}"))
            .unwrap_or_else(|_| self.base_url.clone())
    }

    /// Send a control request and return the parsed business value on success.
    /// Verifies the echoed `rpcId`; returns the dsh `RpcError` on `ok: false`.
    pub async fn call<P, V>(&self, method: &str, rpc_id: RpcId, payload: &P) -> anyhow::Result<V>
    where
        P: serde::Serialize,
        V: DeserializeOwned,
    {
        let body = ClientRequest::new(rpc_id.clone(), method, serde_json::to_value(payload)?);
        let response = self
            .inner
            .post(self.api_url(method))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("transport failure for {method}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "transport failure for {method}: HTTP {status}: {text}"
            ));
        }
        let server: ServerResponse = response
            .json()
            .await
            .with_context(|| format!("invalid server-response for {method}"))?;
        if server.rpcId != rpc_id {
            return Err(anyhow!(
                "rpcId mismatch for {method}: sent {rpc_id}, got {}",
                server.rpcId
            ));
        }
        match server.result {
            crate::rpc_types::RpcResult::Ok { value, .. } => serde_json::from_value::<V>(value)
                .with_context(|| format!("invalid {method} response value")),
            crate::rpc_types::RpcResult::Err { error, .. } => Err(anyhow!("{error}")),
        }
    }

    /// POST a `ClientResponse` to `/api/respond` and return the carrier receipt.
    /// A `not-pending` receipt (late/duplicate respond) is returned as-is, not
    /// an error — the bridge treats it as a no-op.
    pub async fn respond(&self, response: &ClientResponse) -> anyhow::Result<RpcReceipt> {
        let resp = self
            .inner
            .post(self.api_url("respond"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(response)
            .send()
            .await
            .context("transport failure for respond")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "transport failure for respond: HTTP {status}: {text}"
            ));
        }
        let text = resp.text().await.context("respond body read")?;
        tracing::debug!(target: "dsh-bridge::respond", body = %text, "respond receipt raw");
        serde_json::from_str::<RpcReceipt>(&text)
            .with_context(|| format!("invalid respond receipt: {text}"))
    }

    // ---- Typed control-method helpers ----

    pub async fn host_describe(&self, rpc_id: RpcId) -> anyhow::Result<HostDescribeValue> {
        self.call::<HostDescribePayload, HostDescribeValue>(
            "host.describe",
            rpc_id,
            &HostDescribePayload {},
        )
        .await
    }

    pub async fn session_list(&self, rpc_id: RpcId) -> anyhow::Result<SessionListValue> {
        self.call::<SessionListPayload, SessionListValue>(
            "session.list",
            rpc_id,
            &SessionListPayload::default(),
        )
        .await
    }

    pub async fn session_create(
        &self,
        rpc_id: RpcId,
        payload: &SessionCreatePayload,
    ) -> anyhow::Result<SessionCreateValue> {
        self.call("session.create", rpc_id, payload).await
    }

    pub async fn session_prompt(
        &self,
        rpc_id: RpcId,
        payload: &SessionPromptPayload,
    ) -> anyhow::Result<SessionPromptValue> {
        self.call("session.prompt", rpc_id, payload).await
    }

    pub async fn session_cancel(
        &self,
        rpc_id: RpcId,
        payload: &SessionCancelPayload,
    ) -> anyhow::Result<SessionCancelValue> {
        self.call("session.cancel", rpc_id, payload).await
    }

    pub async fn session_history(
        &self,
        rpc_id: RpcId,
        payload: &SessionHistoryPayload,
    ) -> anyhow::Result<SessionHistoryValue> {
        self.call("session.history", rpc_id, payload).await
    }

    pub async fn session_models(
        &self,
        rpc_id: RpcId,
        payload: &SessionModelsPayload,
    ) -> anyhow::Result<Value> {
        // The models catalog is held as opaque JSON (groups/failures shape is
        // rich and not consumed by the bridge in v1 beyond the current selection).
        self.call::<SessionModelsPayload, Value>("session.models", rpc_id, payload)
            .await
    }

    pub async fn session_select_model(
        &self,
        rpc_id: RpcId,
        payload: &SessionSelectModelPayload,
    ) -> anyhow::Result<Value> {
        self.call::<SessionSelectModelPayload, Value>("session.selectModel", rpc_id, payload)
            .await
    }

    pub async fn agent_preset_list(
        &self,
        rpc_id: RpcId,
    ) -> anyhow::Result<crate::rpc_types::AgentPresetListValue> {
        self.call::<crate::rpc_types::AgentPresetListPayload, crate::rpc_types::AgentPresetListValue>(
            "agentPreset.list",
            rpc_id,
            &crate::rpc_types::AgentPresetListPayload {},
        )
        .await
    }

    pub async fn agent_preset_select(
        &self,
        rpc_id: RpcId,
        payload: &crate::rpc_types::AgentPresetSelectPayload,
    ) -> anyhow::Result<crate::rpc_types::AgentPresetSelectValue> {
        self.call("agentPreset.select", rpc_id, payload).await
    }

    // ---- WebSocket event streams ----

    /// Open `GET /api/events.mux` as a WebSocket stream of [`ServerRequest`]s
    /// whose payload is a `MuxFrame`. dsh's `client-connection` plugin requires
    /// a WebSocket upgrade for these paths (a plain GET gets 426). Each WS text
    /// message is a JSON `ServerRequest`; malformed frames are skipped with a
    /// debug log. The stream ends when the host closes the socket or the caller
    /// drops the [`SseStream`].
    pub async fn open_mux(&self) -> anyhow::Result<SseStream> {
        self.open_ws("events.mux").await
    }

    /// Open `GET /api/events.host` as a WebSocket stream of [`ServerRequest`]s
    /// whose payload is a `HostFrame`.
    pub async fn open_host(&self) -> anyhow::Result<SseStream> {
        self.open_ws("events.host").await
    }

    async fn open_ws(&self, path: &str) -> anyhow::Result<SseStream> {
        // dsh serves the event streams over WebSocket, not HTTP SSE. The HTTP
        // URL (`http://...`) maps to `ws://...` (and `https://` to `wss://...`).
        let ws_url = self
            .api_url(path)
            .to_string()
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        // No timeout — streams are long-lived; the caller's shutdown signal
        // aborts by dropping the stream (closing the socket).
        let (stream, _response) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("transport failure for {path}"))?;
        Ok(SseStream::from_ws(stream))
    }
}

/// A parsed event-frame stream. Yields one [`ServerRequest`] per WebSocket
/// text message. Malformed frames (bad JSON, bad envelope) are skipped.
pub struct SseStream {
    inner: std::pin::Pin<Box<dyn Stream<Item = ServerRequest> + Send>>,
}

impl SseStream {
    /// Build a [`SseStream`] from a WebSocket message stream. Each text
    /// message is parsed as a JSON [`ServerRequest`]; binary messages and
    /// parse failures are skipped with a debug log. A transport error or Close
    /// frame ends the stream.
    fn from_ws<S>(ws: S) -> Self
    where
        S: Stream<
                Item = Result<
                    tokio_tungstenite::tungstenite::Message,
                    tokio_tungstenite::tungstenite::Error,
                >,
            > + Send
            + 'static,
    {
        let stream = ws
            .filter_map(|msg| async move {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::debug!(target: "dsh-bridge::ws", error = %err, "ws stream error");
                        return None;
                    }
                };
                match msg {
                    tokio_tungstenite::tungstenite::Message::Text(text) => {
                        // Debug aid for the mojibake hunt: log a content
                        // fingerprint of any frame carrying assistant text so a
                        // corrupted payload can be traced back to its source
                        // (dsh host vs. our own mapping).
                        if text.contains("assistant/chunk") || text.contains("assistant/message") {
                            tracing::debug!(
                                target: "dsh-bridge::ws",
                                bytes = text.len(),
                                fffd = text.matches('\u{FFFD}').count(),
                                latin1 = text.matches('Ã').count(),
                                "assistant frame fingerprint"
                            );
                        }
                        match serde_json::from_str::<ServerRequest>(&text) {
                            Ok(req) => Some(req),
                            Err(err) => {
                                tracing::debug!(target: "dsh-bridge::ws", error = %err, "dropping malformed WS frame");
                                None
                            }
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                        match serde_json::from_slice::<ServerRequest>(&bytes) {
                            Ok(req) => Some(req),
                            Err(err) => {
                                tracing::debug!(target: "dsh-bridge::ws", error = %err, "dropping malformed binary WS frame");
                                None
                            }
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Close(_) => None,
                    _ => None,
                }
            });
        Self {
            inner: Box::pin(stream),
        }
    }

    pub async fn next(&mut self) -> Option<ServerRequest> {
        self.inner.next().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::Message;

    fn frame(rpc_id: &str, session_id: &str) -> Message {
        Message::Text(
            serde_json::json!({
                "type": "server-request",
                "rpcId": rpc_id,
                "method": "session/event",
                "payload": {
                    "type": "session/subscribed",
                    "sessionId": session_id,
                    "lastSeq": 0
                }
            })
            .to_string()
            .into(),
        )
    }

    #[tokio::test]
    async fn ws_stream_parses_frames() {
        let msgs: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> = vec![
            Ok(frame("r1", "s1")),
            Ok(frame("r2", "s2")),
            Ok(Message::Close(None)),
        ];
        let stream = futures::stream::iter(msgs);
        let mut sse = SseStream::from_ws(stream);
        let f1 = sse.next().await.unwrap();
        assert_eq!(f1.rpcId, "r1");
        assert_eq!(f1.payload["sessionId"], "s1");
        let f2 = sse.next().await.unwrap();
        assert_eq!(f2.rpcId, "r2");
        assert_eq!(f2.payload["sessionId"], "s2");
        assert!(sse.next().await.is_none());
    }

    #[tokio::test]
    async fn ws_stream_skips_malformed_frame() {
        let msgs: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> =
            vec![Ok(Message::Text("not-json".into())), Ok(frame("r1", "s1"))];
        let stream = futures::stream::iter(msgs);
        let mut sse = SseStream::from_ws(stream);
        // The malformed frame is skipped; the valid frame arrives.
        let f1 = sse.next().await.unwrap();
        assert_eq!(f1.rpcId, "r1");
        assert!(sse.next().await.is_none());
    }
}
