//! Per-session lifecycle over the harness RPC: the `run_harness_session` entry
//! point mirrors `acp_core::runtime::run_session`'s shape so `SessionHandle`
//! stays transport-agnostic.
//!
//! One session acquires a shared [`HarnessHost`] from the registry, calls
//! `session.create` with the workspace `cwd`, registers a [`SessionSink`] in
//! the `SessionRouter`, and drives `RuntimeCommand`s over the shared HTTP
//! client (direct POSTs — not serialized through the router). Events arrive via
//! the host's router sink (the mux/host SSE loops run on the host's runtime).
//! One in-flight prompt per session; parallel across sessions sharing a host.

use acp_core::{ClientEvent, PermissionBroker, RuntimeCommand, SessionConfig, ShutdownSignal};
use std::sync::Arc;
use std::sync::mpsc;
use uuid::Uuid;
use workspace_model::UserPromptContent;

use crate::host::{HarnessHostRegistry, SessionSink};
use crate::rpc_types::{
    PromptContentPart, PromptMode, SessionCancelPayload, SessionCreatePayload, SessionId,
    SessionPromptPayload,
};

/// Entry point — same shape as `acp_core::runtime::run_session`.
///
/// Runs on the caller's thread (the `SessionHandle` worker thread). The shared
/// SSE loops run on the host's own multi-thread runtime, so this function does
/// not own them and must not block on event delivery — it only drives the
/// command channel and issues control POSTs.
pub fn run_harness_session(
    registry: Arc<HarnessHostRegistry>,
    config: SessionConfig,
    tx_events: mpsc::Sender<ClientEvent>,
    rx_commands: mpsc::Receiver<RuntimeCommand>,
    permission_broker: PermissionBroker,
    shutdown_signal: ShutdownSignal,
) -> anyhow::Result<()> {
    let endpoint = config
        .harness_endpoint
        .clone()
        .ok_or_else(|| anyhow::anyhow!("harness_endpoint is not set"))?;

    // Acquire (or reuse) a shared host for this endpoint.
    let host = registry.acquire(endpoint.clone())?;
    let client = host.client().clone();

    // Create the harness session.
    let create_payload = SessionCreatePayload {
        cwd: Some(config.workspace_root.clone()),
        ..Default::default()
    };
    let create_value = host
        .runtime()
        .block_on(client.session_create(Uuid::new_v4().to_string(), &create_payload))?;
    let session_id: SessionId = create_value.session_id;

    // Register a sink so the host's SSE loops deliver frames for this session.
    let sink = Arc::new(SessionSink::new(tx_events.clone(), permission_broker.clone()));
    sink.set_session_id(session_id.clone());
    host.router().register(session_id.clone(), sink.clone());

    // If resuming, replay history before the live stream delivers events.
    if let Some(resume_id) = config.resume_session_id.clone()
        && !resume_id.is_empty()
    {
        let _ = host
            .runtime()
            .block_on(replay_history(&client, &resume_id, &sink));
    }

    // Emit SessionStarted so the reducer flips to the running state. The dsh
    // `session/subscribed` frame also arrives via mux, but the handle needs an
    // id promptly (it picks the session id off SessionStarted).
    let _ = tx_events.send(ClientEvent::SessionStarted {
        session_id: session_id.clone(),
    });

    // Drive the command channel on this thread. Events flow through the sink
    // on the host's runtime; we only handle commands here.
    let mut inflight_prompt: Option<String> = None;

    for command in rx_commands.iter() {
        if shutdown_signal.is_requested() {
            break;
        }
        match command {
            RuntimeCommand::SendPrompt { prompt, accepted_tx } => {
                if inflight_prompt.is_some() {
                    if let Some(tx) = accepted_tx {
                        let _ = tx.send(Err(anyhow::anyhow!(
                            "a prompt is already in flight for this session"
                        )));
                    }
                    continue;
                }
                let mode = if accepted_tx.is_some() {
                    PromptMode::Steer
                } else {
                    PromptMode::Queue
                };
                let parts: Vec<PromptContentPart> =
                    prompt.into_iter().map(prompt_part_to_wire).collect();
                let payload = SessionPromptPayload {
                    session_id: session_id.clone(),
                    mode,
                    content: parts,
                    client_time_zone: Some(local_timezone()),
                };
                let rpc_id = Uuid::new_v4().to_string();
                let result = host
                    .runtime()
                    .block_on(client.session_prompt(rpc_id.clone(), &payload));
                match result {
                    Ok(_) => {
                        inflight_prompt = Some(rpc_id);
                        if let Some(tx) = accepted_tx {
                            let _ = tx.send(Ok(()));
                        }
                    }
                    Err(err) => {
                        let _ = tx_events.send(ClientEvent::Interrupted {
                            reason: format!("session.prompt failed: {err}"),
                        });
                        if let Some(tx) = accepted_tx {
                            let _ = tx.send(Err(err));
                        }
                    }
                }
            }
            RuntimeCommand::CancelPrompt { reply_tx } => {
                let payload = SessionCancelPayload {
                    session_id: session_id.clone(),
                };
                let result = host
                    .runtime()
                    .block_on(client.session_cancel(Uuid::new_v4().to_string(), &payload));
                inflight_prompt = None;
                if let Some(tx) = reply_tx {
                    let _ = tx.send(result.map(|_| ()));
                }
            }
            RuntimeCommand::ResolveHarnessApproval { rpc_id, result, reply_tx } => {
                let pending = sink.pending_approvals();
                let response = pending.build_response(&sink, &rpc_id, &result);
                let send_result = match response {
                    Some(response) => host.runtime().block_on(client.respond(&response)),
                    None => Err(anyhow::anyhow!("no pending approval/question for id {rpc_id}")),
                };
                // A not-pending receipt (late/duplicate) is a no-op, not an error.
                let send_result = send_result.or_else(|err| {
                    if err.to_string().contains("not-pending") {
                        Ok(crate::rpc_types::RpcReceipt::Accepted { accepted: true })
                    } else {
                        Err(err)
                    }
                });
                let _ = reply_tx.send(send_result.map(|_| ()));
            }
            RuntimeCommand::SetModel { model_id, provider, reply_tx } => {
                let payload = crate::rpc_types::SessionSelectModelPayload {
                    session_id: session_id.clone(),
                    provider: provider.unwrap_or_default(),
                    model: model_id,
                    reasoning_effort: None,
                };
                let result = host.runtime().block_on(
                    client.session_select_model(Uuid::new_v4().to_string(), &payload),
                );
                let events = match result {
                    Ok(_) => Vec::new(),
                    Err(err) => vec![ClientEvent::Interrupted {
                        reason: format!("session.selectModel failed: {err}"),
                    }],
                };
                let _ = reply_tx.send(Ok(events));
            }
            RuntimeCommand::StopTool { reply_tx, .. } => {
                // dsh supports only whole-turn cancel; per-tool stop degrades
                // to session.cancel with a UI note. Map StopTool -> cancel.
                let payload = SessionCancelPayload {
                    session_id: session_id.clone(),
                };
                let _ = host
                    .runtime()
                    .block_on(client.session_cancel(Uuid::new_v4().to_string(), &payload));
                let _ = reply_tx.send(Ok(vec![ClientEvent::Interrupted {
                    reason: "per-tool stop is not supported by the harness backend; turn cancelled"
                        .to_string(),
                }]));
            }
            RuntimeCommand::Shutdown => break,
            // Config-option changes are ACP-specific; the harness backend
            // surfaces config via request/header events instead.
            RuntimeCommand::SetConfigOption { reply_tx, .. } => {
                if let Some(tx) = reply_tx {
                    let _ = tx.send(Ok(Vec::new()));
                }
            }
            // Mode changes are ACP-specific; the harness backend surfaces
            // config via request/header events instead.
            RuntimeCommand::SetMode { reply_tx, .. } => {
                let _ = reply_tx.send(Ok(Vec::new()));
            }
            RuntimeCommand::ResolveCodeBuddyInterruption { reply_tx, .. } => {
                let _ = reply_tx.send(Err(anyhow::anyhow!(
                    "ResolveCodeBuddyInterruption is not supported by the harness backend"
                )));
            }
        }
    }

    // Session exit: unregister from the router and release the host refcount.
    host.router().unregister(&session_id);
    registry.release(&endpoint);
    Ok(())
}

/// Replay a session's history through the mapping layer before the live stream
/// delivers events. Used on resume/switch.
async fn replay_history(
    client: &crate::transport::HttpClient,
    session_id: &SessionId,
    sink: &SessionSink,
) -> anyhow::Result<()> {
    let payload = crate::rpc_types::SessionHistoryPayload {
        session_id: session_id.to_string(),
        before_seq: None,
        max_messages: Some(200),
    };
    let value = client
        .session_history(Uuid::new_v4().to_string(), &payload)
        .await?;
    for entry in value.events {
        if let Ok(event) = serde_json::from_value::<crate::frame::SessionEvent>(entry.event) {
            let view = entry
                .view
                .and_then(|v| serde_json::from_value::<crate::frame::ToolEventView>(v).ok());
            let events = crate::mapping::map_session_event(&event, view.as_ref(), sink);
            for ev in events {
                sink.send(ev);
            }
            sink.last_seq
                .store(event.seq, std::sync::atomic::Ordering::Release);
        }
    }
    Ok(())
}

fn prompt_part_to_wire(part: UserPromptContent) -> PromptContentPart {
    match part {
        UserPromptContent::Text { text } => PromptContentPart::text(text),
        // Other UserPromptContent variants (images, context) are not carried in
        // v1; the harness prompt wire is text-only for now.
        _ => PromptContentPart::text(String::new()),
    }
}

fn local_timezone() -> String {
    // Best-effort IANA timezone; the harness uses this for prompt timestamps.
    std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string())
}
