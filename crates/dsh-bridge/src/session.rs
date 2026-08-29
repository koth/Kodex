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
    //
    // A preset is only sent for a NEW session. The harness fixes the preset at
    // creation and rejects a conflicting preset on resume with
    // `agent-preset-conflict`, so a resume must not carry one — the session's
    // own preset is respected automatically.
    let agent_preset = if config
        .resume_session_id
        .as_ref()
        .is_some_and(|id| !id.is_empty())
    {
        None
    } else {
        config.agent_preset.clone().filter(|p| !p.is_empty())
    };
    let create_payload = SessionCreatePayload {
        cwd: Some(config.workspace_root.clone()),
        session_id: config.resume_session_id.clone().filter(|id| !id.is_empty()),
        agent_preset,
        ..Default::default()
    };
    let workspace_root = config.workspace_root.clone();
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
    // Shared "a prompt turn is in flight" flag. The session thread sets it when
    // a queued prompt is accepted and checks it before queueing the next one;
    // the sink clears it when the turn's `TurnFinished`/`Interrupted` is
    // forwarded to app-core. Without this, the guard would never be cleared
    // (turn completion flows through the sink on the host runtime, not through
    // this command loop) and the next prompt would be silently dropped.
    let inflight = Arc::new(std::sync::atomic::AtomicBool::new(false));
    sink.set_inflight_flag(inflight.clone());
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

    // Persist/restore the session's actual preset so reconnect/resume shows
    // the correct preset instead of falling back to the global default.
    // - New session: dsh acks `session.create` with the actual preset.
    // - Resume: dsh does not echo the preset back and we don't send one (to
    //   avoid `agent-preset-conflict`), but the session's own preset is still
    //   known from the store — re-emit it so the UI restores correctly.
    let mut session_preset: Option<String> = create_value
        .agent_preset
        .as_deref()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            // On resume the store carries the preset; app-core passes it via
            // `config.agent_preset` even though dsh's `session.create` must
            // not receive it.
            config.agent_preset.as_deref().filter(|p| !p.is_empty())
        })
        .map(|p| p.to_string());
    if let Some(preset) = &session_preset {
        let _ = tx_events.send(ClientEvent::SessionConfigValueChanged {
            control_id: "agent_preset".to_string(),
            value_id: preset.clone(),
            value_label: None,
        });
    }

    // Publish the config selectors: fetch `session.models` and `agentPreset.list`
    // and translate them into a single `SessionConfigUpdated` carrying the
    // Model and Mode (agent-preset) controls so the composer dropdowns render.
    // Pass `session_preset` so the Mode control immediately shows the
    // session's actual preset instead of the deployment default. Previously
    // a separate `emit_model_control(None)` call first published the default
    // preset, then `emit_config_controls` corrected it — but during resume the
    // `restore_pending_model_selection` flow sends a `SetModel` whose reply
    // also re-published with `None`, racing the correct value and leaving the
    // Mode control stuck on the default after a session switch-back.
    let models_start = Instant::now();
    emit_config_controls(
        &client,
        &host,
        &session_id,
        session_preset.as_deref(),
        &tx_events,
    );
    tracing::info!(
        target: "dsh-bridge::session",
        elapsed_ms = models_start.elapsed().as_millis() as u64,
        session_id = %session_id,
        "session.models + agentPreset.list published",
    );
    // Publish the `/compact` slash command for sessions whose preset composes
    // the compaction seam (`standard`). The harness publishes no ACP
    // `available_commands_update`, so the bridge synthesizes the one command
    // kodex executes on the user's behalf: the composer renders it in the "/"
    // menu, and sending it routes to the manual-compaction path in app-core
    // (`ForceCompact` → `commands/execute`). `minimal` sessions have neither
    // the command nor auto-compaction, so they get no entry.
    if session_preset.as_deref() == Some("standard") {
        let _ = tx_events.send(ClientEvent::AvailableCommandsUpdated {
            commands: compact_slash_commands(),
        });
    }
    // Declare prompt capabilities. The harness `session/prompt` RPC accepts
    // `mode: "steer"` (an in-flight prompt can be steered mid-turn), so kodex
    // advertises `session_steer`. Workspace file/directory references are
    // carried as text mentions (`@path`) since the harness prompt wire is
    // text-only, so `embedded_context` is enabled and `prompt_part_to_wire`
    // translates `WorkspaceFile` parts into mention text. Image attachments
    // stay disabled until the harness image-attachment path is wired through.
    let _ = tx_events.send(ClientEvent::PromptCapabilitiesUpdated {
        capabilities: workspace_model::PromptInputCapabilities {
            session_steer: true,
            embedded_context: true,
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
    // on the host's runtime; we only handle commands here. The shared
    // `inflight` flag is cleared by the sink when a turn completes
    // (`TurnFinished`/`Interrupted`), so the guard below stays accurate even
    // though turn completion never re-enters this loop.
    use std::sync::atomic::Ordering as AtomicOrdering;

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
                // flight and must not set the inflight flag. A queued prompt
                // starts a fresh turn and is rejected while one is running.
                if inflight.load(AtomicOrdering::Acquire) && !is_steer {
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
                    prompt.into_iter().map(|p| prompt_part_to_wire(p, &workspace_root)).collect();
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
                        // A steer does not start a new turn; leave the inflight
                        // flag as-is so the running turn's completion still
                        // clears it.
                        if !is_steer {
                            inflight.store(true, AtomicOrdering::Release);
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
                        // The prompt never started, so no turn is in flight.
                        inflight.store(false, AtomicOrdering::Release);
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
                inflight.store(false, AtomicOrdering::Release);
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
                if let Some(response) = &response {
                    tracing::info!(
                        target: "dsh-bridge::respond",
                        rpc_id,
                        payload = %serde_json::to_string(response).unwrap_or_default(),
                        "sending respond"
                    );
                }
                let send_result = match response {
                    Some(ref response) => host.runtime().block_on(client.respond(response)),
                    None => Err(anyhow::anyhow!(
                        "no pending approval/question for id {rpc_id}"
                    )),
                };
                // A `bad-response` means dsh rejected the answer payload
                // (validation mismatch) — the pending ask stays open on the
                // host and the turn hangs. Surface it instead of swallowing.
                let send_result = send_result.and_then(|receipt| {
                    if !receipt.accepted() {
                        let reason = match &receipt {
                            crate::rpc_types::RpcReceipt::Rejected { reason, .. } => {
                                reason.clone()
                            }
                            _ => "rejected".to_string(),
                        };
                        tracing::warn!(
                            target: "dsh-bridge::respond",
                            rpc_id,
                            response = %serde_json::to_string(&response).unwrap_or_default(),
                            reason = %reason,
                            "harness rejected respond (question/approval stays pending)"
                        );
                        let _ = sink.send(ClientEvent::MessageChunk {
                            role: workspace_model::MessageRole::System,
                            content: format!("回答未被接受（{reason}），请重试或取消当前轮次。"),
                        });
                        return Err(anyhow::anyhow!("harness respond rejected: {reason}"));
                    }
                    Ok(receipt)
                });
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
                        // the new selection. Pass `session_preset` so the Mode
                        // control preserves the session's actual preset instead
                        // of resetting to the deployment default.
                        let mut refreshed = Vec::new();
                        emit_model_control_into(
                            &client,
                            &host,
                            &session_id,
                            session_preset.as_deref(),
                            &mut refreshed,
                        );
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
            RuntimeCommand::ForceCompact { reply_tx } => {
                // Manual compaction: execute the harness `/compact` command
                // over the typert gateway — spawned on the host runtime so
                // neither this command loop nor the caller (which holds the
                // app mutex) blocks for the run. The compaction is a full LLM
                // summarization (minutes on large contexts) and the harness
                // aborts the command when the HTTP request dies, so the
                // request uses the long `COMMANDS_EXECUTE_TIMEOUT` cap. The
                // outcome reaches the UI through the mux events
                // (`compaction/start` → `compaction/end`, plus the paired
                // `command/done` result mapped in `map_session_event`); the
                // reply only acknowledges the dispatch.
                let compact_client = client.clone();
                let compact_session_id = session_id.clone();
                host.runtime().spawn(async move {
                    let start = Instant::now();
                    let result = compact_client
                        .commands_execute(
                            Uuid::new_v4().to_string(),
                            &compact_session_id,
                            "/compact",
                        )
                        .await;
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    match &result {
                        Ok(None) => tracing::warn!(
                            target: "dsh-bridge::session",
                            elapsed_ms,
                            session_id = %compact_session_id,
                            "manual compaction: /compact command not registered (agent preset has no compaction seam)",
                        ),
                        Ok(Some(execution)) => match &execution.result {
                            crate::rpc_types::CommandsExecuteResult::Success { .. } => {
                                tracing::info!(
                                    target: "dsh-bridge::session",
                                    elapsed_ms,
                                    session_id = %compact_session_id,
                                    command_id = %execution.command_id,
                                    "manual compaction succeeded",
                                );
                            }
                            crate::rpc_types::CommandsExecuteResult::Error { text } => {
                                tracing::warn!(
                                    target: "dsh-bridge::session",
                                    elapsed_ms,
                                    session_id = %compact_session_id,
                                    command_id = %execution.command_id,
                                    error = %text,
                                    "manual compaction failed",
                                );
                            }
                        },
                        Err(err) => {
                            tracing::warn!(
                                target: "dsh-bridge::session",
                                elapsed_ms,
                                session_id = %compact_session_id,
                                error = %err,
                                "commands/execute failed",
                            );
                        }
                    }
                });
                let _ = reply_tx.send(Ok(()));
            }
            RuntimeCommand::ForkSession {
                at_user_turn,
                reply_tx,
            } => {
                // Conversation fork (`session.fork`): a fast control-plane
                // call — resolve the completed-turn boundary from the session
                // history, then POST the fork. Blocking reply: app-core needs
                // the child session id to create the local branch session.
                let result = host
                    .runtime()
                    .block_on(fork_session_at_turn(&client, &session_id, at_user_turn));
                match &result {
                    Ok(child) => tracing::info!(
                        target: "dsh-bridge::session",
                        session_id = %session_id,
                        child_session_id = %child,
                        at_user_turn,
                        "session.fork completed",
                    ),
                    Err(error) => tracing::warn!(
                        target: "dsh-bridge::session",
                        session_id = %session_id,
                        at_user_turn,
                        error = %error,
                        "session.fork failed",
                    ),
                }
                let _ = reply_tx.send(result);
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
                            emit_model_control_into(
                                &client,
                                &host,
                                &session_id,
                                session_preset.as_deref(),
                                &mut refreshed,
                            );
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
            // Mode = dsh agent preset: `agentPreset.select` switches the
            // composition for this session, then re-publish the config
            // controls so the dropdown reflects the new selection.
            RuntimeCommand::SetMode { mode_id, reply_tx } => {
                // The harness only allows switching presets on a blank
                // session; once a turn has started it rejects with
                // `agent-preset-locked`. Detect that up front so we never
                // mutate the shared permission broker on a doomed request
                // (`SessionHandle::set_mode` only skips the broker update on
                // RPC errors, but we want to fail before any RPC).
                let history_payload = crate::rpc_types::SessionHistoryPayload {
                    session_id: session_id.to_string(),
                    before_seq: None,
                    max_messages: Some(1),
                };
                let started = host
                    .runtime()
                    .block_on(client.session_history(
                        Uuid::new_v4().to_string(),
                        &history_payload,
                    ))
                    .map(|value| !value.events.is_empty())
                    .unwrap_or(false);
                if started {
                    let _ = reply_tx.send(Err(anyhow::anyhow!(
                        "该会话已开始，预设已固定。请新建会话并选择目标预设。"
                    )));
                    continue;
                }
                let payload = crate::rpc_types::AgentPresetSelectPayload {
                    session_id: session_id.clone(),
                    agent_preset: mode_id.clone(),
                };
                let events = match host
                    .runtime()
                    .block_on(client.agent_preset_select(Uuid::new_v4().to_string(), &payload))
                {
                    Ok(value) => {
                        // Track the new preset so subsequent
                        // `emit_model_control_into` calls (e.g. from a
                        // SetModel reply) preserve it instead of resetting
                        // the Mode control to the deployment default.
                        session_preset = Some(value.agent_preset.clone());
                        let mut refreshed = Vec::new();
                        emit_model_control_into(
                            &client,
                            &host,
                            &session_id,
                            Some(value.agent_preset.as_str()),
                            &mut refreshed,
                        );
                        refreshed
                    }
                    Err(err) => {
                        // A preset switch is only valid on a blank session;
                        // once a turn has started dsh answers
                        // `agent-preset-locked`. Return an error so
                        // `SessionHandle::set_mode` does not apply the new id
                        // to the shared permission broker, and so the composer
                        // surfaces the failure instead of pretending the
                        // switch worked.
                        if crate::rpc_types::rpc_error_code(&err).as_deref()
                            == Some("agent-preset-locked")
                        {
                            let _ = reply_tx.send(Err(anyhow::anyhow!(
                                "该会话已开始，预设已固定。请新建会话并选择目标预设。"
                            )));
                            continue;
                        }
                        vec![ClientEvent::Interrupted {
                            reason: format!("agentPreset.select failed: {err}"),
                        }]
                    }
                };
                let _ = reply_tx.send(Ok(events));
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
    sink.set_replaying(true);
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
    sink.set_replaying(false);
    Ok(())
}

/// Fork the harness session so the child inherits everything through the end
/// of turn `at_user_turn` (1-based, matching the harness per-session turn
/// counter: one queued `session.prompt` = one turn; steers and out-of-band
/// events like compaction commands do not consume turn numbers).
///
/// The harness fork RPC anchors on an event seq ("first `turn/end` at or after
/// `atSeq`"), while app-core knows only the local turn ordinal — so this walks
/// the session history (tail pages, `beforeSeq` strictly-less paging) and maps
/// the ordinal to the `turn/end` event carrying that turn number.
async fn fork_session_at_turn(
    client: &crate::transport::HttpClient,
    session_id: &SessionId,
    at_user_turn: u64,
) -> anyhow::Result<String> {
    let target_seq = find_completed_turn_end_seq(client, session_id, at_user_turn).await?;
    let payload = crate::rpc_types::SessionForkPayload {
        session_id: session_id.clone(),
        at_seq: Some(target_seq),
    };
    let value = client
        .session_fork(Uuid::new_v4().to_string(), &payload)
        .await
        .map_err(|error| anyhow::anyhow!("分叉会话失败：{error}"))?;
    Ok(value.session_id)
}

/// Walk the session history and return the seq of the `at_user_turn`-th
/// (1-based) `turn/end` event, in ascending seq order. One queued harness
/// prompt = one turn = one `turn/end`; steers and out-of-band events
/// (compaction commands, projections) never produce `turn/end`, so the
/// ordinal matches app-core's local turn count without depending on the
/// harness `data.turn` numbering being contiguous. Errors when the turn never
/// completed (open turn) or the ordinal is past the session's last turn.
async fn find_completed_turn_end_seq(
    client: &crate::transport::HttpClient,
    session_id: &SessionId,
    at_user_turn: u64,
) -> anyhow::Result<u64> {
    const HISTORY_PAGE_MESSAGES: u32 = 200;
    /// Hard walk bound so a pathological history cannot spin the session loop
    /// forever: 250 pages × 200 messages ≫ any real session.
    const MAX_HISTORY_PAGES: usize = 250;

    let mut before_seq: Option<u64> = None;
    let mut turn_end_seqs: Vec<u64> = Vec::new();
    for _ in 0..MAX_HISTORY_PAGES {
        let payload = crate::rpc_types::SessionHistoryPayload {
            session_id: session_id.clone(),
            before_seq,
            max_messages: Some(HISTORY_PAGE_MESSAGES),
        };
        let value = client
            .session_history(Uuid::new_v4().to_string(), &payload)
            .await
            .map_err(|error| anyhow::anyhow!("分叉会话失败：读取会话历史失败：{error}"))?;
        // Pages run tail → head; events arrive ascending within a page. The
        // T-th boundary is only known once the whole log is walked, so just
        // collect and sort at the end.
        let mut oldest_seq: Option<u64> = None;
        for entry in &value.events {
            let Ok(event) = serde_json::from_value::<crate::frame::SessionEvent>(entry.event.clone())
            else {
                continue;
            };
            oldest_seq = Some(oldest_seq.map_or(event.seq, |min| min.min(event.seq)));
            if event.type_tag == "turn/end" {
                turn_end_seqs.push(event.seq);
            }
        }
        if !value.has_more {
            break;
        }
        // `has_more` implies a non-empty page whose oldest event bounds the
        // next window; an empty page means the paging contract broke — bail
        // instead of spinning.
        let Some(seq) = oldest_seq else {
            break;
        };
        before_seq = Some(seq);
    }
    turn_end_seqs.sort_unstable();
    if at_user_turn == 0 {
        return Err(anyhow::anyhow!("无法分叉：无效的轮次序号"));
    }
    turn_end_seqs
        .get((at_user_turn - 1) as usize)
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "无法分叉：第 {at_user_turn} 轮尚未完成（或该消息所在轮次不存在）"
            )
        })
}

/// The slash commands kodex can execute on behalf of a harness session. The
/// harness surfaces no ACP command list, so the bridge publishes the commands
/// it can route itself; `standard` composes the compaction seam, whose
/// `/compact` command runs over `commands/execute`.
fn compact_slash_commands() -> Vec<workspace_model::AvailableCommand> {
    vec![workspace_model::AvailableCommand {
        name: "compact".into(),
        description: "压缩当前会话上下文".into(),
        input_hint: None,
    }]
}

/// Emit both config controls (Model + agent-preset Mode) in one update.
fn emit_config_controls(
    client: &crate::transport::HttpClient,
    host: &crate::host::HarnessHost,
    session_id: &SessionId,
    current_preset: Option<&str>,
    tx_events: &mpsc::Sender<ClientEvent>,
) {
    let mut events = Vec::new();
    emit_model_control_into(client, host, session_id, current_preset, &mut events);
    for event in events {
        let _ = tx_events.send(event);
    }
}

/// Build the config-control `ClientEvent`s (does not send; caller drains).
/// Fetches `session.models` (Model control) and `agentPreset.list` (Mode
/// control) and merges them into a single `SessionConfigUpdated`.
fn emit_model_control_into(
    client: &crate::transport::HttpClient,
    host: &crate::host::HarnessHost,
    session_id: &SessionId,
    current_preset: Option<&str>,
    out: &mut Vec<ClientEvent>,
) {
    let payload = crate::rpc_types::SessionModelsPayload {
        session_id: session_id.to_string(),
    };
    let model_control = match host
        .runtime()
        .block_on(client.session_models(Uuid::new_v4().to_string(), &payload))
    {
        Ok(value) => model_control_from_models(&value),
        // No model catalog yet (e.g. no provider configured): fall through
        // with no Model control so the UI settles instead of spinning forever.
        Err(_) => None,
    };
    let preset_control = preset_control_from_list(client, host, current_preset);
    let mut controls = Vec::new();
    if let Some(control) = model_control {
        controls.push(control);
    }
    if let Some(control) = preset_control {
        controls.push(control);
    }
    out.push(ClientEvent::SessionConfigUpdated {
        state: workspace_model::SessionConfigState {
            hydrated: true,
            controls,
        },
    });
}

/// Build the agent-preset Mode control from `agentPreset.list`. Returns None
/// when the deployment composes no presets (the control is hidden).
fn preset_control_from_list(
    client: &crate::transport::HttpClient,
    host: &crate::host::HarnessHost,
    current_preset: Option<&str>,
) -> Option<workspace_model::SessionConfigControl> {
    let value = host
        .runtime()
        .block_on(client.agent_preset_list(Uuid::new_v4().to_string()))
        .ok()?;
    if value.presets.is_empty() {
        return None;
    }
    // Current selection: the session's own preset (from session.create) wins;
    // otherwise fall back to the deployment default.
    let current_id = current_preset
        .map(|p| p.to_string())
        .or_else(|| {
            value
                .presets
                .iter()
                .find(|p| p.is_default)
                .map(|p| p.id.clone())
        })
        .unwrap_or_else(|| value.presets[0].id.clone());
    let current_label = value
        .presets
        .iter()
        .find(|p| p.id == current_id)
        .map(|p| preset_label(p))
        .unwrap_or_else(|| current_id.clone());
    let choices = value
        .presets
        .iter()
        .map(|p| workspace_model::SessionConfigChoice {
            id: p.id.clone(),
            label: preset_label(p),
            description: p.description.clone(),
            provider: None,
            provider_label: None,
        })
        .collect();
    Some(workspace_model::SessionConfigControl {
        id: "mode".to_string(),
        label: "Mode".to_string(),
        description: Some(
            "切换将开启新会话（dsh 预设仅在会话空白时可切换）".to_string(),
        ),
        category: workspace_model::SessionConfigCategory::Mode,
        // LegacyMode routes the change through `RuntimeCommand::SetMode`,
        // which this backend maps to `agentPreset.select`. (LocalMode would
        // only update the local permission broker and never reach the host.)
        source: workspace_model::SessionConfigSource::LegacyMode,
        current_value_id: current_id,
        current_value_label: current_label,
        choices,
        enabled: true,
    })
}

fn preset_label(p: &crate::rpc_types::AgentPresetEntry) -> String {
    p.name.clone().unwrap_or_else(|| p.id.clone())
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

fn prompt_part_to_wire(part: UserPromptContent, workspace_root: &str) -> PromptContentPart {
    match part {
        UserPromptContent::Text { text } => PromptContentPart::text(text),
        // Workspace file/directory references are translated to a text mention
        // (`@path` / `@dir/`) — the harness prompt wire is text-only, so the
        // reference is carried as a mention the agent can resolve itself.
        UserPromptContent::WorkspaceFile {
            path,
            start_line,
            end_line,
        } => match acp_core::runtime::workspace_reference_to_mention_text(
            workspace_root,
            &path,
            start_line,
            end_line,
        ) {
            Ok(mention) => PromptContentPart::text(mention),
            // Fall back to a bare mention if the path can't be resolved (e.g.
            // removed between attach and send) rather than dropping it silently.
            Err(_) => PromptContentPart::text(format!(
                "@{}",
                path.replace('\\', "/").trim_start_matches('/')
            )),
        },
        // Image attachments are forwarded as image content parts so multimodal
        // harness models can view them natively. Text-only models never reach
        // this path: app-core degrades image blocks to view-model text
        // descriptions (via the `kodex-image` fallback) before dispatching the
        // prompt, so the bridge only sees `Text` parts for them.
        UserPromptContent::Image {
            data,
            mime_type,
            name,
            ..
        } => PromptContentPart::Image {
            media_type: mime_type,
            data,
            name,
        },
        // Other UserPromptContent variants (opaque file blobs) are not carried
        // in v1; the harness prompt wire is text/image-only for now.
        _ => PromptContentPart::text(String::new()),
    }
}

fn local_timezone() -> String {
    // Best-effort IANA timezone; the harness uses this for prompt timestamps.
    std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn text_part_passes_through() {
        let part = prompt_part_to_wire(UserPromptContent::text("hello"), "/tmp");
        match part {
            PromptContentPart::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn workspace_file_part_becomes_mention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();

        let part = prompt_part_to_wire(
            UserPromptContent::workspace_file("src/lib.rs", Some(1), Some(1)),
            root.to_str().unwrap(),
        );
        match part {
            PromptContentPart::Text { text } => assert_eq!(text, "@src/lib.rs#L1"),
            other => panic!("expected text mention, got {other:?}"),
        }
    }

    #[test]
    fn workspace_directory_part_becomes_dir_mention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();

        let part = prompt_part_to_wire(
            UserPromptContent::workspace_file("src", None, None),
            root.to_str().unwrap(),
        );
        match part {
            PromptContentPart::Text { text } => assert_eq!(text, "@src/"),
            other => panic!("expected dir mention, got {other:?}"),
        }
    }

    #[test]
    fn image_part_is_forwarded_as_image_content() {
        let part = prompt_part_to_wire(
            UserPromptContent::image("Zm9v", "image/png", Some("pic.png".to_string())),
            "/tmp",
        );
        match part {
            PromptContentPart::Image {
                media_type,
                data,
                name,
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "Zm9v");
                assert_eq!(name.as_deref(), Some("pic.png"));
            }
            other => panic!("expected image part, got {other:?}"),
        }
    }

    #[test]
    fn sink_clears_inflight_flag_on_turn_finished() {
        // Regression: a turn completes via the event stream (the session
        // thread's command loop never sees TurnFinished), so the sink must
        // clear the shared in-flight flag — otherwise the next queued prompt
        // is silently dropped by the "one prompt per turn" guard.
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;

        let (tx, _rx) = mpsc::channel::<acp_core::ClientEvent>();
        let sink = crate::host::SessionSink::new(tx, acp_core::PermissionBroker::default());
        let flag = std::sync::Arc::new(AtomicBool::new(true));
        sink.set_inflight_flag(flag.clone());
        assert!(flag.load(Ordering::Acquire), "flag should start set");

        // A mid-turn event must NOT clear the flag.
        sink.send(acp_core::ClientEvent::MessageChunk {
            role: workspace_model::MessageRole::Assistant,
            content: "thinking".to_string(),
        });
        assert!(
            flag.load(Ordering::Acquire),
            "non-terminal event must not clear the in-flight flag"
        );

        // TurnFinished clears it.
        sink.send(acp_core::ClientEvent::TurnFinished {
            stop_reason: "end_turn".to_string(),
            detail: None,
        });
        assert!(
            !flag.load(Ordering::Acquire),
            "TurnFinished must clear the in-flight flag so the next prompt is accepted"
        );
    }

    #[test]
    fn sink_clears_inflight_flag_on_interrupted() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;

        let (tx, _rx) = mpsc::channel::<acp_core::ClientEvent>();
        let sink = crate::host::SessionSink::new(tx, acp_core::PermissionBroker::default());
        let flag = std::sync::Arc::new(AtomicBool::new(true));
        sink.set_inflight_flag(flag.clone());

        sink.send(acp_core::ClientEvent::Interrupted {
            reason: "boom".to_string(),
        });
        assert!(
            !flag.load(Ordering::Acquire),
            "Interrupted must clear the in-flight flag"
        );
    }

    #[test]
    fn compact_slash_command_is_published_for_the_standard_preset() {
        let commands = compact_slash_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "compact");
        assert!(!commands[0].description.is_empty());
        assert!(commands[0].input_hint.is_none());
    }
}
