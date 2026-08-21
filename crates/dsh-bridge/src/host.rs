//! Shared `HarnessHost` + `SessionRouter` + `HarnessHostRegistry` (Mode B).
//!
//! One `HarnessHost` owns one `dsh web` process (or one external endpoint), one
//! mux + one host WebSocket connection, a dedicated multi-thread tokio runtime, and a
//! `SessionRouter`. Multiple Kodex `SessionHandle`s targeting the same endpoint
//! share the host: the mux/host WebSocket read loops run on the host's own runtime
//! (outliving any single session), and frames are demuxed by `sessionId` into
//! per-session sinks. Control POSTs go direct over the shared `reqwest` client
//! (not serialized through the router).

use acp_core::{ClientEvent, PermissionBroker};
use dashmap::DashMap;
use futures::StreamExt;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Notify;

use crate::approval::PendingApprovals;
use crate::frame::{HostFrame, MuxFrame};
use crate::rpc_types::{RpcId, ServerRequest, SessionId};
use crate::transport::HttpClient;

/// Bound on the re-baseline fan-out (`session.history` calls) after a stream drop.
/// Matches the design doc's `REBASELINE_CONCURRENCY` default (4) for the
/// typical 1–3 session desktop case.
const REBASELINE_CONCURRENCY: usize = 4;

/// One pending approval/question entry kind, recorded in the session sink so
/// the respond path can route the user's decision back to `/api/respond`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingApprovalKind {
    Approval,
    Question,
}

/// Per-session state held by the `SessionRouter`: the event channel back to the
/// `SessionHandle`, the permission broker, the last-delivered `seq` (for
/// re-baseline), and the pending approval/question table (keyed by the dsh
/// `rpcId`/`approvalId` surfaced to the UI).
pub struct SessionSink {
    pub tx_events: mpsc::Sender<ClientEvent>,
    pub permission_broker: PermissionBroker,
    pub last_seq: AtomicU64,
    /// Pending approvals/questions keyed by the id surfaced to the UI (dsh
    /// `approvalId` for approvals; the question batch's first `question.id` for
    /// questions). The value carries the dsh `rpcId` needed for `/api/respond`
    /// (for questions, the rpcId is on the `question/requested` frame, stored
    /// separately in `question_rpc_ids`).
    pending: Mutex<Vec<PendingEntry>>,
    /// Map UI request id → dsh `rpcId` for question batches (approvals use the
    /// approvalId as both the UI id and the respond payload field; questions
    /// need the server-request rpcId for the respond envelope).
    question_rpc_ids: Mutex<Vec<(String, RpcId)>>,
    /// The dsh session id, set once `session.create` returns.
    session_id: Mutex<Option<SessionId>>,
    /// Set when the session has been removed by the host (`host/session-removed`)
    /// during a stream gap, so re-baseline skips it and marks it Interrupted.
    removed: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub ui_id: String,
    pub kind: PendingApprovalKind,
    pub approval_id: String,
}

impl SessionSink {
    pub fn new(tx_events: mpsc::Sender<ClientEvent>, broker: PermissionBroker) -> Self {
        Self {
            tx_events,
            permission_broker: broker,
            last_seq: AtomicU64::new(0),
            pending: Mutex::new(Vec::new()),
            question_rpc_ids: Mutex::new(Vec::new()),
            session_id: Mutex::new(None),
            removed: AtomicBool::new(false),
        }
    }

    pub fn set_session_id(&self, id: SessionId) {
        if let Ok(mut guard) = self.session_id.lock() {
            *guard = Some(id);
        }
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id.lock().ok().and_then(|g| g.clone())
    }

    pub fn mark_removed(&self) {
        self.removed.store(true, Ordering::Release);
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Acquire)
    }

    pub fn send(&self, event: ClientEvent) {
        // A send error means the SessionHandle dropped its receiver; the router
        // will unregister it shortly. Log and continue.
        let _ = self.tx_events.send(event);
    }

    pub fn record_pending_approval(&self, approval_id: String, kind: PendingApprovalKind) {
        if let Ok(mut guard) = self.pending.lock() {
            guard.push(PendingEntry {
                ui_id: approval_id.clone(),
                kind,
                approval_id,
            });
        }
    }

    pub fn record_pending_question(&self, ui_id: String) {
        // The rpcId is attached later when the session loop receives the
        // `question/requested` ServerRequest (it has the rpcId); for now we
        // record a placeholder so the UI id is known.
        if let Ok(mut guard) = self.pending.lock() {
            guard.push(PendingEntry {
                ui_id: ui_id.clone(),
                kind: PendingApprovalKind::Question,
                approval_id: ui_id,
            });
        }
    }

    pub fn attach_question_rpc_id(&self, ui_id: String, rpc_id: RpcId) {
        if let Ok(mut guard) = self.question_rpc_ids.lock() {
            guard.push((ui_id, rpc_id));
        }
    }

    /// Remove a pending question batch once the harness reports it resolved.
    pub fn clear_pending_question(&self, ui_id: &str) {
        if let Ok(mut guard) = self.question_rpc_ids.lock() {
            guard.retain(|(id, _)| id != ui_id);
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|e| !(e.kind == PendingApprovalKind::Question && e.ui_id == ui_id));
        }
    }

    /// The UI-facing question id (`request_id`) for a pending batch, resolved
    /// from its rpcId — `question/resolved` names the batch by rpcId, while
    /// `ToolPermissionResolved` must carry the id the UI registered.
    pub fn question_ui_id_for_rpc_id(&self, rpc_id: &str) -> Option<String> {
        self.question_rpc_ids.lock().ok().and_then(|guard| {
            guard
                .iter()
                .find(|(_, id)| id == rpc_id)
                .map(|(ui_id, _)| ui_id.clone())
        })
    }

    pub fn pending_approvals(&self) -> PendingApprovals {
        let entries = self.pending.lock().map(|g| g.clone()).unwrap_or_default();
        let qrpc = self
            .question_rpc_ids
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        PendingApprovals::from_entries(entries, qrpc)
    }
}

/// Demuxes host-level WebSocket frames by `sessionId` to per-session sinks.
/// Frames whose `sessionId` is not registered are dropped with a debug log
/// (not errors — e.g. a session created by another client of the same host).
pub struct SessionRouter {
    sinks: DashMap<SessionId, Arc<SessionSink>>,
    /// Set when the host startup probe failed. Any sink registered afterwards
    /// is immediately told (the probe may have raced ahead of registration).
    probe_error: Mutex<Option<String>>,
}

impl SessionRouter {
    pub fn new() -> Self {
        Self {
            sinks: DashMap::new(),
            probe_error: Mutex::new(None),
        }
    }

    /// Record a startup-probe failure so late-registered sinks are told too.
    pub fn set_probe_error(&self, error: String) {
        if let Ok(mut guard) = self.probe_error.lock() {
            *guard = Some(error);
        }
    }

    pub fn register(&self, session_id: SessionId, sink: Arc<SessionSink>) {
        if let Ok(guard) = self.probe_error.lock()
            && let Some(error) = guard.as_ref()
        {
            sink.send(ClientEvent::Interrupted {
                reason: format!("harness host unreachable: {error}"),
            });
        }
        self.sinks.insert(session_id, sink);
    }

    pub fn unregister(&self, session_id: &SessionId) {
        self.sinks.remove(session_id);
    }

    pub fn get(&self, session_id: &SessionId) -> Option<Arc<SessionSink>> {
        self.sinks.get(session_id).map(|r| r.clone())
    }

    /// Snapshot all live sessions for re-baseline after a stream drop.
    pub fn live_sessions(&self) -> Vec<(SessionId, Arc<SessionSink>)> {
        self.sinks
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Broadcast a host-global event to every sink (e.g. a fatal reopen
    /// failure). Used when the mux stream cannot be reopened.
    pub fn broadcast(&self, event: ClientEvent) {
        for entry in self.sinks.iter() {
            entry.value().send(event.clone());
        }
    }
}

impl Default for SessionRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// One harness host: a shared `HttpClient`, a dedicated multi-thread `Runtime`,
/// the mux + host WebSocket read loops, and a `SessionRouter`. Refcounted by active
/// sessions; the last drop closes the WebSocket streams, disposes the runtime, and
/// kills the spawned `dsh web` process (if Kodex spawned it).
pub struct HarnessHost {
    endpoint: String,
    client: HttpClient,
    router: Arc<SessionRouter>,
    runtime: Runtime,
    /// Notified when the WebSocket loops should stop (last session dropped or
    /// shutdown). Drives cancellation of the read loops.
    stop: Arc<Notify>,
    /// Whether Kodex owns a child attached to this host. Only owned children
    /// are killed on last-drop; external endpoints are never terminated.
    spawned: AtomicBool,
    /// Optional child handle for a Kodex-spawned `dsh web` process.
    child: Mutex<Option<crate::process::DshChild>>,
    /// Host version recorded by the startup probe (for diagnostics/branching).
    version: Mutex<Option<String>>,
    /// Prevents double-teardown.
    torn_down: AtomicBool,
}

impl std::fmt::Debug for HarnessHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessHost")
            .field("endpoint", &self.endpoint)
            .field("spawned", &self.spawned.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl HarnessHost {
    /// Connect to an external endpoint (no process spawn).
    pub fn connect(endpoint: String) -> anyhow::Result<Arc<Self>> {
        Self::build(endpoint, false)
    }

    fn build(endpoint: String, spawned: bool) -> anyhow::Result<Arc<Self>> {
        let client = HttpClient::new(&endpoint)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build host runtime: {e}"))?;
        let host = Arc::new(Self {
            endpoint,
            client,
            router: Arc::new(SessionRouter::new()),
            runtime,
            stop: Arc::new(Notify::new()),
            spawned: AtomicBool::new(spawned),
            child: Mutex::new(None),
            version: Mutex::new(None),
            torn_down: AtomicBool::new(false),
        });
        // Startup probe: confirm the endpoint is a harness host and record the
        // version. Fail fast with a diagnostic instead of hanging on SSE.
        let probed = host.clone();
        host.runtime.spawn(async move {
            match probed.client.host_describe(uuid::Uuid::new_v4().to_string()).await {
                Ok(value) => {
                    let version = value.version.clone();
                    if let Ok(mut guard) = probed.version.lock() {
                        *guard = Some(version.clone());
                    }
                    tracing::info!(target: "dsh-bridge::host", endpoint = %probed.endpoint, version = %version, "connected to harness host");
                }
                Err(err) => {
                    tracing::warn!(target: "dsh-bridge::host", endpoint = %probed.endpoint, error = %err, "startup probe failed");
                    probed.router.set_probe_error(err.to_string());
                    // Broadcast an Interrupted to any already-registered sinks.
                    probed.router.broadcast(ClientEvent::Interrupted {
                        reason: format!("harness host unreachable: {err}"),
                    });
                }
            }
        });
        // Start the SSE read loops on the host's own runtime.
        let host_for_mux = host.clone();
        host.runtime
            .spawn(async move { host_for_mux.run_mux_loop().await });
        let host_for_host = host.clone();
        host.runtime
            .spawn(async move { host_for_host.run_host_loop().await });
        Ok(host)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn client(&self) -> &HttpClient {
        &self.client
    }

    pub fn router(&self) -> &Arc<SessionRouter> {
        &self.router
    }

    /// Whether the startup probe has failed (endpoint unreachable / not a
    /// harness host). Late-registered sinks are told via the router.
    pub fn probe_failed(&self) -> bool {
        self.router
            .probe_error
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Attach a Kodex-spawned child so it is killed on last-drop.
    pub fn attach_child(&self, child: crate::process::DshChild) {
        self.spawned.store(true, Ordering::Release);
        if let Ok(mut guard) = self.child.lock() {
            *guard = Some(child);
        }
    }

    /// Mux SSE read loop. Reconnects with bounded backoff on stream end; on
    /// each reconnect, re-baselines every live session from its `last_seq` via
    /// `session.history` with bounded concurrency before resuming the live
    /// stream. A failed reopen (after retries) fails all sessions with
    /// `Interrupted`.
    async fn run_mux_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.stop.notified() => return,
                result = self.client.open_mux() => {
                    let mut stream = match result {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(target: "dsh-bridge::host::mux", error = %err, "mux open failed; backing off");
                            if !self.backoff_or_fail().await { return; }
                            continue;
                        }
                    };
                    // On (re)open, re-baseline all live sessions from last_seq.
                    self.rebaseline_all().await;
                    loop {
                        tokio::select! {
                            _ = self.stop.notified() => return,
                            frame = stream.next() => {
                                let Some(req) = frame else { break; };
                                self.dispatch_mux(&req);
                            }
                        }
                    }
                    // Stream ended — backoff and reopen (unless stopped).
                    tracing::debug!(target: "dsh-bridge::host::mux", "mux stream ended; reconnecting");
                }
            }
        }
    }

    /// Host SSE read loop. Reconnects with backoff; routes per-session frames
    /// and honors `host/session-removed` (marks the sink removed so re-baseline
    /// skips it). Host-global frames are ignored in v1.
    async fn run_host_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.stop.notified() => return,
                result = self.client.open_host() => {
                    let mut stream = match result {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(target: "dsh-bridge::host::host", error = %err, "host stream open failed; backing off");
                            if !self.backoff_or_fail().await { return; }
                            continue;
                        }
                    };
                    loop {
                        tokio::select! {
                            _ = self.stop.notified() => return,
                            frame = stream.next() => {
                                let Some(req) = frame else { break; };
                                self.dispatch_host(&req);
                            }
                        }
                    }
                    tracing::debug!(target: "dsh-bridge::host::host", "host stream ended; reconnecting");
                }
            }
        }
    }

    /// Dispatch one mux `ServerRequest`: parse the payload as a `MuxFrame`,
    /// demux by `sessionId`, map to `ClientEvent`(s), send to the matched sink.
    /// Unmatched/unknown frames are dropped with a debug log.
    fn dispatch_mux(&self, req: &ServerRequest) {
        let frame: MuxFrame = match serde_json::from_value(req.payload.clone()) {
            Ok(f) => f,
            Err(err) => {
                tracing::debug!(target: "dsh-bridge::host::mux", error = %err, "dropping unparseable mux frame");
                return;
            }
        };
        // Attach question rpcId: question/requested frames carry the batch's
        // stable rpcId on the ServerRequest envelope; record it on the sink so
        // the respond path can echo it.
        if let MuxFrame::QuestionRequested {
            session_id,
            questions,
            ..
        } = &frame
        {
            if let Some(sink) = self.router.get(session_id) {
                if let Some(first) = questions.first() {
                    sink.attach_question_rpc_id(first.id.clone(), req.rpcId.clone());
                }
            }
        }
        // question/resolved names the batch by rpcId, while the UI tracks it by
        // the request id (the first question id): translate before mapping so
        // ToolPermissionResolved reaches the panel that is waiting on it.
        if let MuxFrame::QuestionResolved {
            session_id,
            question_rpc_id,
            ..
        } = &frame
            && let Some(sink) = self.router.get(session_id)
            && let Some(ui_id) = sink.question_ui_id_for_rpc_id(question_rpc_id)
        {
            sink.clear_pending_question(&ui_id);
            sink.send(ClientEvent::ToolPermissionResolved {
                id: ui_id,
                outcome: match frame {
                    MuxFrame::QuestionResolved { outcome, .. } => outcome.clone(),
                    _ => unreachable!(),
                },
            });
            return;
        }
        let Some(session_id) = frame.session_id() else {
            tracing::debug!(target: "dsh-bridge::host::mux", "mux frame without sessionId; dropping");
            return;
        };
        let Some(sink) = self.router.get(session_id) else {
            tracing::debug!(target: "dsh-bridge::host::mux", session_id, "mux frame for unregistered session; dropping");
            return;
        };
        let mapped = crate::mapping::map_mux_frame(&frame, &sink);
        for event in mapped.events {
            sink.send(event);
        }
    }

    fn dispatch_host(&self, req: &ServerRequest) {
        let frame: HostFrame = match serde_json::from_value(req.payload.clone()) {
            Ok(f) => f,
            Err(err) => {
                tracing::debug!(target: "dsh-bridge::host::host", error = %err, "dropping unparseable host frame");
                return;
            }
        };
        // Honor host/session-removed: mark the sink so re-baseline skips it.
        if let HostFrame::HostSessionRemoved { session_id } = &frame {
            if let Some(sink) = self.router.get(session_id) {
                sink.mark_removed();
            }
        }
        let Some(session_id) = frame.session_id() else {
            // Host-global frame — ignored in v1 (workspace/archived/remote).
            return;
        };
        let Some(sink) = self.router.get(session_id) else {
            return;
        };
        let mapped = crate::mapping::map_host_frame(&frame);
        for event in mapped.events {
            sink.send(event);
        }
    }

    /// Re-baseline every live session from its `last_seq` via `session.history`,
    /// bounded with `buffer_unordered(REBASELINE_CONCURRENCY)`. Per-session
    /// failure isolates (that session gets `Interrupted`); others continue.
    async fn rebaseline_all(&self) {
        let sessions = self.router.live_sessions();
        if sessions.is_empty() {
            return;
        }
        let client = self.client.clone();
        let mut futures = futures::stream::iter(sessions.into_iter().map(
            |(session_id, sink)| {
                let client = client.clone();
                async move {
                    if sink.is_removed() {
                        return;
                    }
                    let from_seq = sink.last_seq.load(Ordering::Acquire);
                    let payload = crate::rpc_types::SessionHistoryPayload {
                        session_id: session_id.clone(),
                        before_seq: Some(from_seq + 1),
                        max_messages: None,
                    };
                    match client
                        .session_history(uuid::Uuid::new_v4().to_string(), &payload)
                        .await
                    {
                        Ok(value) => {
                            for entry in value.events {
                                let event_json = entry.event;
                                let view_json = entry.view;
                                if let Ok(event) =
                                    serde_json::from_value::<crate::frame::SessionEvent>(event_json)
                                {
                                    let view = view_json
                                        .and_then(|v| serde_json::from_value::<crate::frame::ToolEventView>(v).ok());
                                    let events = crate::mapping::map_session_event(
                                        &event, view.as_ref(), &sink,
                                    );
                                    for ev in events {
                                        sink.send(ev);
                                    }
                                    sink.last_seq.store(event.seq, Ordering::Release);
                                }
                            }
                        }
                        Err(err) => {
                            tracing::warn!(target: "dsh-bridge::host::rebaseline", session_id = %session_id, error = %err, "history fetch failed; isolating session");
                            sink.send(ClientEvent::Interrupted {
                                reason: format!("history replay failed: {err}"),
                            });
                        }
                    }
                }
            },
        ))
        .buffer_unordered(REBASELINE_CONCURRENCY);
        while let Some(_) = futures.next().await {}
    }

    /// Bounded backoff before reopening a stream. Returns `false` if the host
    /// is stopping (caller should exit the loop). After a few failed retries,
    /// broadcasts `Interrupted` to all sessions but keeps trying.
    async fn backoff_or_fail(&self) -> bool {
        if self.stop_inner() {
            return false;
        }
        tokio::select! {
            _ = self.stop.notified() => return false,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        true
    }

    fn stop_inner(&self) -> bool {
        // The host is stopping if the router is empty AND teardown began. We
        // rely on the `stop` Notify for explicit cancellation; this is a
        // best-effort check to avoid spinning when no sessions remain.
        self.torn_down.load(Ordering::Acquire)
    }

    /// Tear down the host: stop SSE loops, dispose runtime, kill spawned child.
    /// Called when the last session drops its refcount.
    pub fn teardown(&self) {
        if self.torn_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.stop.notify_waiters();
        // Kill a Kodex-spawned process (stdin EOF grace then terminate); never
        // kill an external endpoint.
        if self.spawned.load(Ordering::Acquire) {
            if let Ok(mut guard) = self.child.lock() {
                if let Some(child) = guard.take() {
                    let _ = crate::process::kill_child(child);
                }
            }
        }
    }
}

impl Drop for HarnessHost {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Registry of shared `HarnessHost`s keyed by endpoint URL. A second session
/// targeting the same endpoint reuses the existing host instead of spawning a
/// new process. Held by `app-core` so the registry outlives any single session.
pub struct HarnessHostRegistry {
    hosts: Mutex<Vec<(String, Arc<HarnessHost>)>>,
}

#[derive(Clone)]
pub struct HarnessHostRegistryHandle {
    inner: Arc<HarnessHostRegistry>,
}

impl HarnessHostRegistry {
    pub fn new() -> Self {
        Self {
            hosts: Mutex::new(Vec::new()),
        }
    }

    /// Acquire (or create) a `HarnessHost` for `endpoint`. If `spawn` is true
    /// and no endpoint is given, the caller is expected to have spawned the
    /// process and pass its discovered endpoint; this method connects to it.
    pub fn acquire(&self, endpoint: String) -> anyhow::Result<Arc<HarnessHost>> {
        if let Ok(guard) = self.hosts.lock() {
            for (url, host) in guard.iter() {
                if url == &endpoint {
                    return Ok(host.clone());
                }
            }
        }
        let host = HarnessHost::connect(endpoint.clone())?;
        if let Ok(mut guard) = self.hosts.lock() {
            guard.push((endpoint, host.clone()));
        }
        Ok(host)
    }

    /// Release a refcount on the host for `endpoint`. The last release tears
    /// down the host (closes SSE, disposes runtime, kills spawned process).
    pub fn release(&self, endpoint: &str) {
        let mut to_drop = None;
        if let Ok(mut guard) = self.hosts.lock() {
            guard.retain(|(url, host)| {
                if url == endpoint {
                    // Arc strong count: 1 in the registry + however many
                    // sessions. When only the registry entry remains, drop it.
                    if Arc::strong_count(host) <= 1 {
                        to_drop = Some(host.clone());
                        return false;
                    }
                }
                true
            });
        }
        if let Some(host) = to_drop {
            // Drop outside the lock to avoid deadlock with the host's own
            // teardown (which may join runtime tasks).
            host.teardown();
        }
    }

    /// Whether a host for `endpoint` is registered and still alive (i.e. the
    /// registry entry has not been torn down by a last-session release).
    pub fn host_alive(&self, endpoint: &str) -> bool {
        self.hosts
            .lock()
            .map(|guard| guard.iter().any(|(url, _)| url == endpoint))
            .unwrap_or(false)
    }
}

impl Default for HarnessHostRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessHostRegistryHandle {
    pub fn new(registry: Arc<HarnessHostRegistry>) -> Self {
        Self { inner: registry }
    }

    pub fn acquire(&self, endpoint: String) -> anyhow::Result<Arc<HarnessHost>> {
        self.inner.acquire(endpoint)
    }

    pub fn release(&self, endpoint: &str) {
        self.inner.release(endpoint);
    }
}

// silence unused-import warnings for types referenced only in docs/comments
#[allow(dead_code)]
fn _unused(_: Value, _: RpcId) {}
