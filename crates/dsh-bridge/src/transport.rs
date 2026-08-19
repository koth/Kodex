//! HTTP + SSE transport for the dsh host RPC.
//!
//! Control plane: `POST /api/<method>` with a [`ClientRequest`] body, parsing
//! the [`ServerResponse`] and verifying the echoed `rpcId` (mirrors
//! `AbstractApiClient.callUnary` in
//! `deepseek-harness/packages/host/apiproxy/src/fetch/client.ts`).
//!
//! Event plane: `GET /api/events.mux` and `GET /api/events.host` as SSE
//! (`text/event-stream`, `data: <json>\n\n` frames). Frames are split on
//! `\n\n`, `data:` lines joined, parsed as [`ServerRequest`] and the payload
//! narrowed to the frame union `F` by the caller. A malformed frame is logged
//! and skipped — one corrupt frame must not kill the stream (mirrors dsh's own
//! `readSse`).

use anyhow::{Context, anyhow};
use futures::{Stream, StreamExt, TryStreamExt};
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
    pub async fn call<P, V>(
        &self,
        method: &str,
        rpc_id: RpcId,
        payload: &P,
    ) -> anyhow::Result<V>
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
            return Err(anyhow!("transport failure for {method}: HTTP {status}: {text}"));
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
            crate::rpc_types::RpcResult::Ok { value, .. } => {
                serde_json::from_value::<V>(value)
                    .with_context(|| format!("invalid {method} response value"))
            }
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
            return Err(anyhow!("transport failure for respond: HTTP {status}: {text}"));
        }
        resp.json::<RpcReceipt>()
            .await
            .context("invalid respond receipt")
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

    // ---- SSE event streams ----

    /// Open `GET /api/events.mux` as an SSE stream of [`ServerRequest`]s whose
    /// payload is a `MuxFrame`. Each `data:` frame is parsed; malformed frames
    /// are skipped with a debug log. The stream ends when the host closes it or
    /// the caller drops the [`SseStream`].
    pub async fn open_mux(&self) -> anyhow::Result<SseStream> {
        self.open_sse("events.mux").await
    }

    /// Open `GET /api/events.host` as an SSE stream of [`ServerRequest`]s whose
    /// payload is a `HostFrame`.
    pub async fn open_host(&self) -> anyhow::Result<SseStream> {
        self.open_sse("events.host").await
    }

    async fn open_sse(&self, path: &str) -> anyhow::Result<SseStream> {
        // No timeout for streams — they are long-lived; rely on the caller's
        // shutdown signal to abort.
        let response = self
            .inner
            .get(self.api_url(path))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .timeout(Duration::MAX)
            .send()
            .await
            .with_context(|| format!("transport failure for {path}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("transport failure for {path}: HTTP {status}: {text}"));
        }
        let bytes = response
            .bytes_stream()
            .map_err(|err| std::io::Error::other(err.to_string()));
        Ok(SseStream::new(bytes))
    }
}

/// A parsed SSE frame stream. Yields one [`ServerRequest`] per `data:` event.
/// Malformed frames (bad JSON, bad envelope) are skipped.
pub struct SseStream {
    inner: std::pin::Pin<Box<dyn Stream<Item = ServerRequest> + Send>>,
}

impl SseStream {
    fn new<S>(bytes: S) -> Self
    where
        S: Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send + 'static,
    {
        // Reassemble SSE event framing over a chunked byte stream by unfolding
        // over `(byte_stream, buffer)`: each step pulls one chunk, appends it to
        // the buffer, and emits zero or more complete `data:` events (split on
        // `\n\n`). Within each event, `data:` lines are joined per the SSE spec
        // (multiple data lines concatenate with `\n`); comment lines (`:`) and
        // unknown fields are ignored. dsh emits one `data:` line per frame, but
        // we follow the spec so a future host change does not break us.
        let byte_stream = Box::pin(bytes);
        let stream = futures::stream::unfold(
            (byte_stream, String::new()),
            |(mut bytes, mut buffer)| async move {
                loop {
                    // Emit any already-complete events in the buffer first.
                    if let Some(idx) = buffer.find("\n\n") {
                        let event = buffer.drain(..idx).collect::<String>();
                        let _ = buffer.drain(..2);
                        if let Some(data) = extract_data_lines(&event) {
                            return Some((data, (bytes, buffer)));
                        }
                        // comment-only / empty event: loop to keep draining.
                        continue;
                    }
                    // No complete event; pull the next chunk.
                    match bytes.next().await {
                        Some(Ok(chunk)) => {
                            buffer.push_str(&String::from_utf8_lossy(&chunk));
                            // loop to re-scan for `\n\n`.
                        }
                        Some(Err(err)) => {
                            tracing::debug!(target: "dsh-bridge::sse", error = %err, "sse byte stream error");
                            // keep draining the buffer, then end on next pass.
                            if buffer.is_empty() {
                                return None;
                            }
                            // force a flush by appending a separator, then loop.
                            buffer.push_str("\n\n");
                        }
                        None => {
                            // Stream ended; flush any trailing partial event.
                            if buffer.is_empty() {
                                return None;
                            }
                            let data = extract_data_lines(&buffer);
                            buffer.clear();
                            return data.map(|d| (d, (bytes, buffer)));
                        }
                    }
                }
            },
        )
        .filter_map(|data: String| async move {
            match serde_json::from_str::<ServerRequest>(&data) {
                Ok(req) => Some(req),
                Err(err) => {
                    tracing::debug!(target: "dsh-bridge::sse", error = %err, "dropping malformed SSE frame");
                    None
                }
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

/// Join the `data:` lines of one SSE event into the frame's JSON text.
/// Returns `None` for comment-only or empty events (no `data:` line).
fn extract_data_lines(event: &str) -> Option<String> {
    let mut data_parts: Vec<&str> = Vec::new();
    for line in event.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix(':') {
            // SSE comment line — ignore (dsh sends `: connected` on open).
            let _ = rest;
            continue;
        }
        if let Some(rest) = line.strip_prefix("data: ") {
            data_parts.push(rest);
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_parts.push(rest);
        }
        // Other SSE fields (id:, event:, retry:) are ignored.
    }
    if data_parts.is_empty() {
        None
    } else {
        Some(data_parts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_data_single_line() {
        let event = "data: {\"type\":\"server-request\"}";
        assert_eq!(
            extract_data_lines(event).as_deref(),
            Some("{\"type\":\"server-request\"}")
        );
    }

    #[test]
    fn extract_data_ignores_comment() {
        let event = ": connected\ndata: {\"x\":1}";
        assert_eq!(extract_data_lines(event).as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn extract_data_multi_line_joins() {
        let event = "data: line1\ndata: line2";
        assert_eq!(extract_data_lines(event).as_deref(), Some("line1\nline2"));
    }

    #[test]
    fn extract_data_empty_returns_none() {
        assert!(extract_data_lines(": keepalive").is_none());
        assert!(extract_data_lines("").is_none());
    }

    #[tokio::test]
    async fn sse_stream_parses_frames() {
        // Simulate a chunked SSE byte stream with two frames split across chunks.
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
            Ok(bytes::Bytes::from_static(
                b": connected\n\ndata: {\"type\":\"server-request\",\"rpcId\":\"r1\",\"method\":\"session/event\",\"payload\":{\"type\":\"session/subscribed\",\"sessionId\":\"s1\",\"lastSeq\":0}}\n\n",
            )),
            Ok(bytes::Bytes::from_static(
                b"data: {\"type\":\"server-request\",\"rpcId\":\"r2\",\"method\":\"session/event\",\"payload\":{\"type\":\"session/subscribed\",\"sessionId\":\"s2\",\"lastSeq\":2}}\n\n",
            )),
        ];
        let stream = futures::stream::iter(chunks);
        let mut sse = SseStream::new(stream);
        let f1 = sse.next().await.unwrap();
        assert_eq!(f1.rpcId, "r1");
        assert_eq!(f1.payload["sessionId"], "s1");
        let f2 = sse.next().await.unwrap();
        assert_eq!(f2.rpcId, "r2");
        assert_eq!(f2.payload["sessionId"], "s2");
        assert!(sse.next().await.is_none());
    }

    #[tokio::test]
    async fn sse_stream_skips_malformed_frame() {
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![Ok(bytes::Bytes::from_static(
            b"data: not-json\n\ndata: {\"type\":\"server-request\",\"rpcId\":\"r1\",\"method\":\"m\",\"payload\":{}}\n\n",
        ))];
        let stream = futures::stream::iter(chunks);
        let mut sse = SseStream::new(stream);
        // The malformed frame is skipped; the valid frame arrives.
        let f1 = sse.next().await.unwrap();
        assert_eq!(f1.rpcId, "r1");
        assert!(sse.next().await.is_none());
    }
}
