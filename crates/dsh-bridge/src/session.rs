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
use std::time::Instant;
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

    // Create (or resume) the harness session. When resuming, pass the stored
    // dsh session id: the harness answers an explicit-id `session.create` by
    // resuming the persisted agent with its full model context, and a retry
    // with the same id+cwd returns the same session. Without the id, dsh
    // mints a fresh blank session and every later prompt lands in it with no
    // prior context.
    let boot_start = Instant::now();
    let create_payload = SessionCreatePayload {
        cwd: Some(config.workspace_root.clone()),
        session_id: config.resume_session_id.clone().filter(|id| !id.is_empty()),
        ..Default::default()
    };
    let create_value = host
        .runtime()
        .block_on(client.session_create(Uuid::new_v4().to_string(), &create_payload))?;
    let session_id: SessionId = create_value.session_id;
    tracing::info!(
        target: "dsh-bridge::session",
        elapsed_ms = boot_start.elapsed().as_millis() as u64,
        session_id = %session_id,
        "session.create completed",
    );

    // Register a sink so the host's SSE loops deliver frames for this session.
    let sink = Arc::new(SessionSink::new(
        tx_events.clone(),
        permission_broker.clone(),
    ));
    sink.set_session_id(session_id.clone());
    host.router().register(session_id.clone(), sink.clone());

    // If resuming, replay history before the live stream delivers events. The
    // resumed dsh session already carries the full model context; this replay
    // only rebuilds the UI transcript.
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

    // Publish the model selector: fetch `session.models` and translate it into
    // a `SessionConfigUpdated` carrying a Model control so the composer's
    // model dropdown renders instead of spinning.
    let models_start = Instant::now();
    emit_model_control(&client, &host, &session_id, &tx_events);
    tracing::info!(
        target: "dsh-bridge::session",
        elapsed_ms = models_start.elapsed().as_millis() as u64,
        session_id = %session_id,
        "session.models published",
    );
    // Declare prompt capabilities. The harness `session/prompt` RPC accepts
    // `mode: "steer"` (an in-flight prompt can be steered mid-turn), so kodex
    // advertises `session_steer`. Image and embedded-context stay disabled
    // until the harness image-attachment path is wired through.
    let _ = tx_events.send(ClientEvent::PromptCapabilitiesUpdated {
        capabilities: workspace_model::PromptInputCapabilities {
            session_steer: true,
            ..Default::default()
        },
    });

    // Drive the command channel on this thread. Events flow through the sink
    // on the host's runtime; we only handle commands here.
    let mut inflight_prompt: Option<String> = None;
    tracing::info!(
        target: "dsh-bridge::session",
        boot_elapsed_ms = boot_start.elapsed().as_millis() as u64,
        session_id = %session_id,
        "command loop ready",
    );

    for command in rx_commands.iter() {
        if shutdown_signal.is_requested() {
            break;
        }
        match command {
            RuntimeCommand::SendPrompt {
                prompt,
                accepted_tx,
            } => {
                let is_steer = accepted_tx.is_some();
                // A steer prompt is delivered to the in-flight turn via the
                // harness `session/prompt` `mode: "steer"` RPC — it does NOT
                // start a new turn, so it is allowed while a prompt is in
                // flight and must not replace `inflight_prompt`. A queued
                // prompt starts a fresh turn and is rejected while one is
                // running.
                if inflight_prompt.is_some() && !is_steer {
                    if let Some(tx) = accepted_tx {
                        let _ = tx.send(Err(anyhow::anyhow!(
                            "a prompt is already in flight for this session"
                        )));
                    }
                    continue;
                }
                let mode = if is_steer {
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
                let prompt_start = Instant::now();
                let result = host
                    .runtime()
                    .block_on(client.session_prompt(rpc_id.clone(), &payload));
                match result {
                    Ok(_) => {
                        tracing::info!(
                            target: "dsh-bridge::session",
                            elapsed_ms = prompt_start.elapsed().as_millis() as u64,
                            session_id = %session_id,
                            steer = is_steer,
                            "session.prompt accepted",
                        );
                        // A steer does not start a new turn; keep the existing
                        // inflight prompt id so the running turn's completion
                        // still clears it.
                        if !is_steer {
                            inflight_prompt = Some(rpc_id);
                        }
                        if let Some(tx) = accepted_tx {
                            let _ = tx.send(Ok(()));
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "dsh-bridge::session",
                            elapsed_ms = prompt_start.elapsed().as_millis() as u64,
                            session_id = %session_id,
                            error = %err,
                            "session.prompt failed",
                        );
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
            RuntimeCommand::ResolveHarnessApproval {
                rpc_id,
                result,
                reply_tx,
            } => {
                let respond_start = Instant::now();
                let pending = sink.pending_approvals();
                let response = pending.build_response(&sink, &rpc_id, &result);
                let send_result = match response {
                    Some(response) => host.runtime().block_on(client.respond(&response)),
                    None => Err(anyhow::anyhow!(
                        "no pending approval/question for id {rpc_id}"
                    )),
                };
                // A not-pending receipt (late/duplicate) is a no-op, not an error.
                let send_result = send_result.or_else(|err| {
                    if err.to_string().contains("not-pending") {
                        Ok(crate::rpc_types::RpcReceipt::Accepted { accepted: true })
                    } else {
                        Err(err)
                    }
                });
                match &send_result {
                    Ok(_) => tracing::info!(
                        target: "dsh-bridge::session",
                        elapsed_ms = respond_start.elapsed().as_millis() as u64,
                        session_id = %session_id,
                        "respond accepted",
                    ),
                    Err(err) => tracing::warn!(
                        target: "dsh-bridge::session",
                        elapsed_ms = respond_start.elapsed().as_millis() as u64,
                        session_id = %session_id,
                        rpc_id = %rpc_id,
                        error = %err,
                        "respond failed",
                    ),
                }
                let _ = reply_tx.send(send_result.map(|_| ()));
            }
            RuntimeCommand::SetModel {
                model_id,
                provider,
                reply_tx,
            } => {
                let (model, provider) = match decode_model_value(&model_id, provider.clone()) {
                    Some(decoded) => decoded,
                    None => (model_id, provider.unwrap_or_default()),
                };
                let payload = crate::rpc_types::SessionSelectModelPayload {
                    session_id: session_id.clone(),
                    provider,
                    model,
                    reasoning_effort: None,
                };
                let result = host
                    .runtime()
                    .block_on(client.session_select_model(Uuid::new_v4().to_string(), &payload));
                let events = match result {
                    Ok(_) => {
                        // Re-publish the model selector so the dropdown reflects
                        // the new selection.
                        let mut refreshed = Vec::new();
                        emit_model_control_into(&client, &host, &session_id, &mut refreshed);
                        refreshed
                    }
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
            // Config-option changes: the composer's model dropdown sends a
            // Model control change → `session.selectModel`; other controls
            // are no-ops (harness surfaces config via request/header).
            RuntimeCommand::SetConfigOption {
                config_id,
                value_id,
                provider,
                reply_tx,
            } => {
                let events = if config_id == "model" {
                    let (model, provider) = match decode_model_value(&value_id, provider) {
                        Some(decoded) => decoded,
                        None => {
                            // Fall back to treating the value as a bare model id.
                            (value_id, String::new())
                        }
                    };
                    let payload = crate::rpc_types::SessionSelectModelPayload {
                        session_id: session_id.clone(),
                        provider,
                        model,
                        reasoning_effort: None,
                    };
                    match host
                        .runtime()
                        .block_on(client.session_select_model(Uuid::new_v4().to_string(), &payload))
                    {
                        Ok(_) => {
                            let mut refreshed = Vec::new();
                            emit_model_control_into(&client, &host, &session_id, &mut refreshed);
                            refreshed
                        }
                        Err(err) => vec![ClientEvent::Interrupted {
                            reason: format!("session.selectModel failed: {err}"),
                        }],
                    }
                } else {
                    Vec::new()
                };
                if let Some(tx) = reply_tx {
                    let _ = tx.send(Ok(events));
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

/// Fetch `session.models` and send a `SessionConfigUpdated` carrying a Model
/// control, so the composer's model dropdown renders the dsh model catalog.
fn emit_model_control(
    client: &crate::transport::HttpClient,
    host: &crate::host::HarnessHost,
    session_id: &SessionId,
    tx_events: &mpsc::Sender<ClientEvent>,
) {
    let mut events = Vec::new();
    emit_model_control_into(client, host, session_id, &mut events);
    for event in events {
        let _ = tx_events.send(event);
    }
}

/// Build the model-control `ClientEvent`s (does not send; caller drains).
fn emit_model_control_into(
    client: &crate::transport::HttpClient,
    host: &crate::host::HarnessHost,
    session_id: &SessionId,
    out: &mut Vec<ClientEvent>,
) {
    let payload = crate::rpc_types::SessionModelsPayload {
        session_id: session_id.to_string(),
    };
    let Ok(value) = host
        .runtime()
        .block_on(client.session_models(Uuid::new_v4().to_string(), &payload))
    else {
        // No model catalog yet (e.g. no provider configured): emit a hydrated
        // empty config so the UI settles instead of spinning forever.
        out.push(ClientEvent::SessionConfigUpdated {
            state: workspace_model::SessionConfigState {
                hydrated: true,
                controls: Vec::new(),
            },
        });
        return;
    };
    if let Some(control) = model_control_from_models(&value) {
        out.push(ClientEvent::SessionConfigUpdated {
            state: workspace_model::SessionConfigState {
                hydrated: true,
                controls: vec![control],
            },
        });
    } else {
        out.push(ClientEvent::SessionConfigUpdated {
            state: workspace_model::SessionConfigState {
                hydrated: true,
                controls: Vec::new(),
            },
        });
    }
}

/// Translate the dsh `session.models` response into a Model `SessionConfigControl`.
fn model_control_from_models(
    value: &serde_json::Value,
) -> Option<workspace_model::SessionConfigControl> {
    let current = value.get("current")?;
    let current_provider = current.get("provider")?.as_str()?.to_string();
    let current_model = current.get("model")?.as_str()?.to_string();

    let mut choices = Vec::new();
    let mut current_value_id = String::new();
    if let Some(groups) = value.get("groups").and_then(serde_json::Value::as_array) {
        for group in groups {
            let provider = group
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let provider_label = group
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&provider)
                .to_string();
            if let Some(models) = group.get("models").and_then(serde_json::Value::as_array) {
                for model in models {
                    let model_id = model
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let model_name = model
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&model_id)
                        .to_string();
                    let encoded = format!("kodex-provider/{provider}/{model_id}");
                    if provider == current_provider && model_id == current_model {
                        current_value_id = encoded.clone();
                    }
                    choices.push(workspace_model::SessionConfigChoice {
                        id: encoded,
                        label: model_name,
                        description: None,
                        provider: Some(provider.clone()),
                        provider_label: Some(provider_label.clone()),
                    });
                }
            }
        }
    }
    if current_value_id.is_empty() {
        // The current model may not be in the catalog (e.g. a provider that
        // reports no groups); surface it so the dropdown shows the active
        // selection even when the choice list is empty.
        current_value_id = format!("kodex-provider/{current_provider}/{current_model}");
        if !choices.iter().any(|choice| choice.id == current_value_id) {
            choices.push(workspace_model::SessionConfigChoice {
                id: current_value_id.clone(),
                label: current_model.clone(),
                description: None,
                provider: Some(current_provider.clone()),
                provider_label: Some(current_provider.clone()),
            });
        }
    }
    if choices.is_empty() {
        return None;
    }
    Some(workspace_model::SessionConfigControl {
        id: "model".to_string(),
        label: "Model".to_string(),
        description: None,
        category: workspace_model::SessionConfigCategory::Model,
        source: workspace_model::SessionConfigSource::SessionModel,
        current_value_id,
        current_value_label: current_model,
        choices,
        enabled: true,
    })
}

/// Decode a model selection value sent by the composer: either
/// `kodex-provider/<provider>/<model>` or a bare model id.
fn decode_model_value(value_id: &str, provider: Option<String>) -> Option<(String, String)> {
    if let Some(rest) = value_id.strip_prefix("kodex-provider/") {
        let mut parts = rest.splitn(2, '/');
        let provider = parts.next()?.to_string();
        let model = parts.next()?.to_string();
        return Some((model, provider));
    }
    Some((value_id.to_string(), provider.unwrap_or_default()))
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
