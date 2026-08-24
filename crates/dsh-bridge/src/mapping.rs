//! Map dsh `MuxFrame`/`HostFrame` variants into Kodex [`ClientEvent`]s.
//!
//! The mapping layer preserves the fidelity the dsh web UI receives: assistant
//! text and reasoning chunks, tool calls/results with render intent
//! (`ToolEventView`), plans (`todo/write`), turn endings, session config, and
//! approvals/questions. Unrepresentable view data is serialized into
//! `raw_output` JSON (recoverable, not dropped). Per the design doc, no raw
//! harness types leak to the frontend — translation stops at `ClientEvent`.

use acp_core::ClientEvent;
use serde_json::Value;
use uuid::Uuid;
use workspace_model::{
    AgentPlanEntry, AgentPlanEntryPriority, AgentPlanEntryStatus, DiffHunk, DiffLine, DiffLineKind,
    MessageRole, PermissionInputOption, PermissionInputQuestion, PermissionInputRequest,
    PermissionOption, TerminalOutput, UsageEvent, UsageEventScope, UsageTokenBreakdown,
};

use crate::frame::{
    AssistantChunkData, AssistantMessageData, ContentBlock, HostFrame, MuxFrame, SessionEvent,
    StreamChunk, TodoItem, TokenUsage, ToolCallData, ToolCallView, ToolEventView, ToolResultData,
    ToolResultView, TurnEndReason,
};
use crate::host::{PendingApprovalKind, SessionSink};

/// Outcome of mapping one frame: zero or more [`ClientEvent`]s to emit.
#[derive(Default)]
pub struct MappedEvents {
    pub events: Vec<ClientEvent>,
}

impl MappedEvents {
    fn single(event: ClientEvent) -> Self {
        Self {
            events: vec![event],
        }
    }
    fn many(events: Vec<ClientEvent>) -> Self {
        Self { events }
    }
}

/// Map a `MuxFrame` (already demuxed to the owning session by the router) into
/// [`ClientEvent`]s. `seq` of the embedded `SessionEvent` updates the sink's
/// `last_seq` so SSE reconnection can re-baseline from the exact gap.
///
/// `sink` is taken by `&SessionSink` so the mapping layer can record pending
/// approval/question ids for the bridge's respond path; it does **not** send
/// events to the sink (the caller does, after mapping).
pub fn map_mux_frame(frame: &MuxFrame, sink: &SessionSink) -> MappedEvents {
    match frame {
        MuxFrame::SessionEvent { event, view, .. } => {
            let mut events = map_session_event(event, view.as_ref(), sink);
            // Advance last_seq after mapping so re-baseline resumes from the gap.
            sink.last_seq
                .store(event.seq, std::sync::atomic::Ordering::Release);
            MappedEvents::many(events.drain(..).collect())
        }
        MuxFrame::SessionSubscribed { last_seq, .. } => {
            // Seed last_seq from the subscription baseline (lastSeq = last
            // delivered seq; the next event is lastSeq + 1).
            sink.last_seq.store(
                (*last_seq).max(0) as u64,
                std::sync::atomic::Ordering::Release,
            );
            MappedEvents::default()
        }
        MuxFrame::ApprovalRequested {
            approval_id,
            tool_name,
            reason,
            ..
        } => {
            sink.record_pending_approval(approval_id.clone(), PendingApprovalKind::Approval);
            MappedEvents::single(ClientEvent::ToolPermissionRequest {
                id: approval_id.clone(),
                name: tool_name.clone(),
                options: approval_options(),
                details: reason.clone(),
                input: None,
            })
        }
        MuxFrame::ApprovalResolved {
            approval_id,
            outcome,
            ..
        } => MappedEvents::single(ClientEvent::ToolPermissionResolved {
            id: approval_id.clone(),
            outcome: outcome.clone(),
        }),
        MuxFrame::QuestionRequested { questions, .. } => {
            // The bridge answers one ask() as a batch via the question's rpcId.
            // Use the first question's id as the request id surfaced to the UI;
            // the full batch is stored in the sink for the respond path.
            let request_id = questions
                .first()
                .map(|q| q.id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            sink.record_pending_question(request_id.clone());
            sink.record_question_order(
                request_id.clone(),
                questions.iter().map(|q| q.id.clone()).collect(),
            );
            let input = PermissionInputRequest {
                questions: questions
                    .iter()
                    .map(|q| PermissionInputQuestion {
                        id: q.id.clone(),
                        header: q.header.clone().unwrap_or_default(),
                        question: q.question.clone(),
                        is_other: false,
                        is_secret: false,
                        multi_select: q.multi_select.unwrap_or(false),
                        options: q
                            .options
                            .iter()
                            .flatten()
                            .map(|o| PermissionInputOption {
                                label: o.label.clone(),
                                description: o.description.clone().unwrap_or_default(),
                            })
                            .collect(),
                    })
                    .collect(),
            };
            MappedEvents::single(ClientEvent::ToolPermissionRequest {
                id: request_id,
                name: "user_question".to_string(),
                options: question_options(),
                details: questions
                    .first()
                    .and_then(|q| q.detail.clone())
                    .or_else(|| questions.first().map(|q| q.question.clone())),
                input: Some(input),
            })
        }
        MuxFrame::QuestionResolved {
            question_rpc_id,
            outcome,
            ..
        } => MappedEvents::single(ClientEvent::ToolPermissionResolved {
            id: question_rpc_id.clone(),
            outcome: outcome.clone(),
        }),
        MuxFrame::SessionProjection { key, value, .. } => {
            if key == "title" {
                if let Some(title) = value.as_str() {
                    return MappedEvents::single(ClientEvent::SessionTitleUpdated {
                        title: title.to_string(),
                    });
                }
                return MappedEvents::default();
            }
            // dsh token-meter projections (see @deepseek-ai/dsh-token-meter):
            //   contextPressure — { pressureTokens?, projectedTokens?, contextWindow? }
            //     the harness's real context occupancy; `projectedTokens` already
            //     reacts to compaction immediately, so feeding it here makes the
            //     UI context bar track the harness instead of the cumulative-token
            //     estimate in the reducer.
            //   tokenUsage — { uncachedInputTokens, outputTokens, cacheReadTokens,
            //     cacheWriteTokens } — durable cumulative provider usage for the
            //     whole session, replacing the per-turn-delta estimate.
            if key == "contextPressure" {
                if let Some(event) = context_pressure_usage_event(&value) {
                    return MappedEvents::single(event);
                }
                return MappedEvents::default();
            }
            if key == "tokenUsage" {
                if let Some(event) = token_usage_projection_event(&value) {
                    return MappedEvents::single(event);
                }
                return MappedEvents::default();
            }
            MappedEvents::default()
        }
        MuxFrame::StreamError { error } => {
            tracing::warn!(target: "dsh-bridge::mapping", error = %error, "mux stream error frame");
            MappedEvents::single(ClientEvent::Interrupted {
                reason: format!("harness stream error: {error}"),
            })
        }
        // session/queue, session/jobs, and unknown frames are not represented
        // in ClientEvent in v1; ignore (debug-logged by the router).
        MuxFrame::SessionQueue { .. } | MuxFrame::SessionJobs { .. } | MuxFrame::Other => {
            MappedEvents::default()
        }
    }
}

/// Map a `HostFrame` (demuxed by `sessionId` where present).
pub fn map_host_frame(frame: &HostFrame) -> MappedEvents {
    match frame {
        HostFrame::HostAgentError { message, .. } => {
            MappedEvents::single(ClientEvent::Interrupted {
                reason: format!("harness agent error: {message}"),
            })
        }
        HostFrame::HostSessionStatus { running: false, .. } => {
            // A session that stopped running without a turn/end (host-side
            // failure) surfaces as Interrupted so the UI can react.
            MappedEvents::single(ClientEvent::Interrupted {
                reason: "harness session stopped".to_string(),
            })
        }
        HostFrame::StreamError { error } => {
            tracing::warn!(target: "dsh-bridge::mapping", error = %error, "host stream error frame");
            MappedEvents::single(ClientEvent::Interrupted {
                reason: format!("harness host stream error: {error}"),
            })
        }
        // session-added/removed, workspace-*, archived-*, remote-event are not
        // represented in v1 (host-global frames are ignored or broadcast per
        // the design doc's open question).
        _ => MappedEvents::default(),
    }
}

/// Map a `SessionEvent` (+ optional `ToolEventView`) into [`ClientEvent`]s.
pub fn map_session_event(
    event: &SessionEvent,
    view: Option<&ToolEventView>,
    _sink: &SessionSink,
) -> Vec<ClientEvent> {
    match event.type_tag.as_str() {
        "assistant/chunk" => {
            let data: Option<AssistantChunkData> = event.data();
            match data.map(|d| d.chunk) {
                Some(StreamChunk::TextDelta { text, .. }) => vec![ClientEvent::MessageChunk {
                    role: MessageRole::Assistant,
                    content: text,
                }],
                Some(StreamChunk::ReasoningDelta { text, .. }) => {
                    vec![
                        ClientEvent::ThinkingActivity { active: true },
                        ClientEvent::ThinkingChunk { text },
                    ]
                }
                Some(StreamChunk::Usage { usage }) => vec![usage_event(&usage)],
                _ => Vec::new(),
            }
        }
        "assistant/message" => {
            // Live path: the assistant text was already streamed via
            // `assistant/chunk` text-deltas, so re-emitting the finalized
            // message's text blocks would duplicate every paragraph — consume
            // only the usage rollup. History-replay path: no chunks were
            // streamed this run, so emit the text blocks here (exactly once
            // per message; replays skip already-delivered seqs).
            let data: Option<AssistantMessageData> = event.data();
            let mut out = Vec::new();
            if let Some(data) = data {
                if _sink.is_replaying() {
                    for block in &data.message.content {
                        if let ContentBlock::Text { text } = block {
                            out.push(ClientEvent::MessageChunk {
                                role: MessageRole::Assistant,
                                content: text.clone(),
                            });
                        }
                    }
                }
                if let Some(usage) = &data.usage {
                    out.push(usage_event(usage));
                }
            }
            out
        }
        "tool/call" => {
            let data: Option<ToolCallData> = event.data();
            let (name, call_id, raw_input) = match data {
                Some(d) => (d.name, d.call_id, d.arguments.clone()),
                None => (String::new(), String::new(), String::new()),
            };
            let raw_input_value = serde_json::from_str::<Value>(&raw_input).ok();
            let (kind, summary) = match view {
                Some(ToolEventView::Call { view }) => {
                    let kind = match view {
                        ToolCallView::Terminal(_) => "execute".to_string(),
                        ToolCallView::Diff(_) => "edit".to_string(),
                        // dsh renders file tools (view / str_replace / search...)
                        // with the generic card, which carries no `kind`. Infer
                        // the semantic kind from the tool name so the UI routes
                        // them to the read/edit surfaces instead of Shell.
                        ToolCallView::Generic(g) => g
                            .kind
                            .clone()
                            .unwrap_or_else(|| infer_tool_kind(&name, raw_input_value.as_ref())),
                        // Unknown cards (e.g. a `read` call card added by a
                        // newer dsh) still need kind inference so the UI does
                        // not fall back to the shell surface.
                        ToolCallView::Other => {
                            infer_tool_kind(&name, raw_input_value.as_ref())
                        }
                    };
                    let summary = view
                        .title()
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| name.clone());
                    (kind, summary)
                }
                _ => (
                    infer_tool_kind(&name, raw_input_value.as_ref()),
                    name.clone(),
                ),
            };
            vec![ClientEvent::ToolStarted {
                id: call_id,
                parent_id: None,
                name: name.clone(),
                kind,
                summary,
                is_subagent: false,
                raw_input: raw_input_value
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_default()),
            }]
        }
        "tool/result" => {
            let data: Option<ToolResultData> = event.data();
            let call_id = data
                .as_ref()
                .and_then(|d| d.message.content.first())
                .and_then(|b| match b {
                    ContentBlock::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let mut out = Vec::new();

            // Diff views → one ToolDiff per file (before ToolCompleted).
            if let Some(ToolEventView::Result { view }) = view {
                if let ToolResultView::Diff(diff_view) = view {
                    for fd in &diff_view.diffs {
                        out.push(ClientEvent::ToolDiff {
                            id: call_id.clone(),
                            path: fd.path.clone(),
                            old_text: fd.old_text.clone(),
                            new_text: fd.new_text.clone(),
                        });
                    }
                }
            }

            let (outcome, terminal_output, raw_output) = match (data.as_ref(), view) {
                (Some(d), Some(ToolEventView::Result { view })) => render_result(d, view),
                (Some(d), None) => (result_outcome(d), None, result_text(d)),
                _ => ("completed".to_string(), None, None),
            };

            if data.as_ref().is_some_and(|d| d.error.is_some()) {
                out.push(ClientEvent::ToolFailed {
                    id: call_id,
                    name: None,
                    error: data
                        .as_ref()
                        .and_then(|d| d.error.as_ref())
                        .map(|e| format!("{}: {}", e.name, e.code))
                        .unwrap_or_else(|| "tool error".to_string()),
                    raw_output,
                    terminal_output,
                });
            } else {
                out.push(ClientEvent::ToolCompleted {
                    id: call_id,
                    name: None,
                    outcome,
                    raw_output,
                    terminal_output,
                });
            }
            out
        }
        "todo/write" => {
            let data: Option<TodoWriteData> = event.data();
            let entries = data
                .map(|d| d.todos)
                .unwrap_or_default()
                .into_iter()
                .map(todo_to_plan_entry)
                .collect();
            vec![ClientEvent::PlanUpdated { entries }]
        }
        "turn/end" => {
            let data: Option<TurnEndData> = event.data();
            let stop_reason = data
                .as_ref()
                .map(|d| turn_end_kind_to_stop_reason(&d.reason.kind))
                .unwrap_or_else(|| "end_turn".to_string());
            // For an upstream LLM failure (kind `error`), surface the real
            // `LlmFailure` (message / code / HTTP status) as detail so the
            // UI refusal notice can name the actual cause (e.g. `429`).
            let detail = data.as_ref().and_then(|d| {
                if d.reason.kind == "error" {
                    d.reason.rest.get("error").and_then(|v| {
                        serde_json::from_value::<LlmFailure>(v.clone())
                            .ok()
                            .as_ref()
                            .and_then(llm_failure_detail)
                    })
                } else {
                    None
                }
            });
            vec![ClientEvent::TurnFinished {
                stop_reason,
                detail,
            }]
        }
        "request/header" => {
            // The full header/config is rich; in v1 the model selector is
            // published from `session.models` by `emit_model_control`. Emitting
            // an empty `SessionConfigUpdated` here would wipe the model control
            // (the reducer replaces `session_config` wholesale), so drop the
            // frame to keep the dropdown populated.
            Vec::new()
        }
        // dsh compaction lifecycle (see @deepseek-ai/dsh-compaction/types):
        // `compaction/start` and `compaction/end` are log-only session events
        // that bracket a context compaction. Map them to the same
        // `ContextCompactionStarted`/`ContextCompacted` notices CodeBuddy uses
        // so the UI shows "正在压缩上下文" → "上下文已压缩". The occupancy drop
        // itself arrives separately via the `contextPressure` projection, whose
        // `projectedTokens` reacts to compaction immediately.
        "compaction/start" => {
            let data: Option<CompactionStartData> = event.data();
            let compaction_id = data.and_then(|d| d.compaction_id);
            vec![ClientEvent::ContextCompactionStarted {
                message: compaction_id
                    .map(|id| format!("正在压缩上下文（{id}）"))
                    .unwrap_or_else(|| "正在压缩上下文".to_string()),
            }]
        }
        "compaction/end" => {
            let data: Option<CompactionEndData> = event.data();
            let message = match data.and_then(|d| d.error) {
                Some(error) if !error.trim().is_empty() => {
                    format!("上下文压缩未完成：{error}")
                }
                _ => "上下文已压缩".to_string(),
            };
            vec![ClientEvent::ContextCompacted { message }]
        }
        // turn/start, step/start, step/end, user/message, request/context,
        // session/end-seed, compaction/summary, compaction/prune: log-only or
        // surface metadata not represented in ClientEvent in v1. Ignorable per
        // dsh's merge-extensibility guard.
        _ => Vec::new(),
    }
}

#[derive(serde::Deserialize)]
struct CompactionStartData {
    #[serde(default, rename = "compactionId")]
    compaction_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct CompactionEndData {
    #[serde(default)]
    error: Option<String>,
}

#[derive(serde::Deserialize)]
struct TodoWriteData {
    #[serde(default)]
    todos: Vec<TodoItem>,
}

#[derive(serde::Deserialize)]
struct TurnEndData {
    reason: TurnEndReason,
}

/// The `error` payload on a `turn/end` of kind `error` — mirrors dsh's
/// `LlmFailure` (`@deepseek-ai/dsh-llm/types`): `{ message, code, status?,
/// providerRetryAfterMs?, requestId? }`. Only the human-facing fields are
/// narrowed; the rest stay opaque.
#[derive(serde::Deserialize)]
struct LlmFailure {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    status: Option<serde_json::Number>,
}

/// Build a short, user-facing detail string from a harness `LlmFailure`.
/// Includes the HTTP status (e.g. `429`) and message when present, so the
/// Kodex refusal notice can surface the real upstream cause instead of the
/// generic wording. Returns `None` only when the payload carried no usable
/// text at all.
fn llm_failure_detail(failure: &LlmFailure) -> Option<String> {
    let message = failure
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let code = failure
        .code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let status = failure
        .status
        .as_ref()
        .and_then(serde_json::Number::as_u64)
        .map(|s| s.to_string());
    match (status.as_deref(), code.as_deref(), message) {
        (Some(s), Some(c), Some(m)) => Some(format!("HTTP {s} ({c}): {m}")),
        (Some(s), Some(c), None) => Some(format!("HTTP {s} ({c})")),
        (Some(s), None, Some(m)) => Some(format!("HTTP {s}: {m}")),
        (Some(s), None, None) => Some(format!("HTTP {s}")),
        (None, Some(c), Some(m)) => Some(format!("{c}: {m}")),
        (None, Some(c), None) => Some(c.to_string()),
        (None, None, Some(m)) => Some(m.to_string()),
        (None, None, None) => None,
    }
}

/// Map a dsh `turn/end` reason kind to Kodex's `TurnFinished` stop reason
/// vocabulary. Mirrors dsh's own `turnEndToStopReason`: `completed`→`end_turn`,
/// `max-tokens`→`max_tokens`, `interrupted`→`cancelled`, `aborted`/`blocked`→
/// `end_turn`. `error` (an upstream LLM failure) maps to `refusal` so the UI
/// surfaces the friendly "上游请求失败/被拒绝/限流" notice instead of a raw
/// `error` stop reason. The real `LlmFailure` (message / code / HTTP status)
/// is carried alongside as `TurnFinished.detail` by the `turn/end` handler.
fn turn_end_kind_to_stop_reason(kind: &str) -> String {
    match kind {
        "completed" => "end_turn".to_string(),
        "max-tokens" => "max_tokens".to_string(),
        "interrupted" => "cancelled".to_string(),
        "error" => "refusal".to_string(),
        _ => "end_turn".to_string(),
    }
}

fn approval_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            id: "allowed-once".to_string(),
            label: "Allow once".to_string(),
            kind: "allow_once".to_string(),
        },
        PermissionOption {
            id: "rejected".to_string(),
            label: "Reject".to_string(),
            kind: "reject_once".to_string(),
        },
    ]
}

/// Options for a `question/requested` input form. The workbench's question
/// panel locates the submit/cancel affordances by id (`submit`/`cancel`); the
/// ids are UI markers only — `build_harness_approval_result` builds the answer
/// from `input_response` and ignores the option id.
fn question_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            id: "submit".to_string(),
            label: "Submit".to_string(),
            kind: "allow_once".to_string(),
        },
        PermissionOption {
            id: "cancel".to_string(),
            label: "Cancel".to_string(),
            kind: "reject_once".to_string(),
        },
    ]
}

fn todo_to_plan_entry(todo: TodoItem) -> AgentPlanEntry {
    let status = match todo.status.as_str() {
        "in_progress" => AgentPlanEntryStatus::InProgress,
        "completed" => AgentPlanEntryStatus::Completed,
        _ => AgentPlanEntryStatus::Pending,
    };
    AgentPlanEntry {
        id: None,
        content: todo.content,
        priority: AgentPlanEntryPriority::Medium,
        status,
    }
}

/// Infer the semantic tool kind (`read` / `edit` / `execute` / `search`) from
/// the dsh tool name when the generic call view carries no explicit `kind`.
/// The workbench routes on `kind`: `read`/`search` render as exploration
/// cards, `edit` renders the diff/变更 surface, `execute` renders the shell
/// surface. Unknown tools stay empty so they keep the generic presentation.
fn infer_tool_kind(name: &str, raw_input: Option<&Value>) -> String {
    let lower = name.trim().to_lowercase();
    let normalized = lower.replace(['_', '-'], " ");
    let matches_any = |needles: &[&str]| {
        needles.iter().any(|needle| {
            normalized == *needle
                || normalized.starts_with(&format!("{needle} "))
                || normalized.ends_with(&format!(" {needle}"))
                || normalized.contains(&format!(" {needle} "))
        })
    };

    // Edit tools: replace/patch/write-shaped names, or any tool whose input
    // carries an old/new text pair (str_replace-style arguments).
    if matches_any(&["edit", "str replace", "replace", "patch", "apply patch", "write", "create"])
    {
        return "edit".to_string();
    }
    if let Some(input) = raw_input {
        let has_old = input.get("old_string").is_some() || input.get("oldString").is_some();
        let has_new = input.get("new_string").is_some() || input.get("newString").is_some();
        if has_old && has_new {
            return "edit".to_string();
        }
    }

    if matches_any(&["view", "read", "open", "cat", "get file"]) {
        return "read".to_string();
    }
    if matches_any(&["search", "grep", "glob", "find", "list", "ls", "query"]) {
        return "search".to_string();
    }
    if matches_any(&["bash", "shell", "exec", "run", "terminal", "command", "cmd"]) {
        return "execute".to_string();
    }
    String::new()
}

fn render_result(
    data: &ToolResultData,
    view: &ToolResultView,
) -> (String, Option<TerminalOutput>, Option<String>) {
    match view {
        ToolResultView::Terminal(t) => {
            let outcome = if t.exit_code == Some(0) {
                "completed".to_string()
            } else {
                "failed".to_string()
            };
            let term = Some(TerminalOutput {
                exit_code: t.exit_code,
                output: t.output.clone().unwrap_or_default(),
            });
            (outcome, term, None)
        }
        ToolResultView::Diff(_) => {
            // Diffs already emitted as ToolDiff events; the completed card
            // carries the model-facing result text as raw_output.
            ("completed".to_string(), None, result_text(data))
        }
        ToolResultView::Read(v) => {
            // Read cards carry `{ path, lines: [{ number, text }] }` — render
            // them as numbered file content so the exploration card shows the
            // actual file instead of raw JSON.
            let rendered = render_read_view(v);
            (result_outcome(data), None, rendered.or_else(|| result_text(data)))
        }
        ToolResultView::Search(v) | ToolResultView::Web(v) => {
            // Unrepresentable structured views → raw_output JSON (recoverable).
            (
                result_outcome(data),
                None,
                Some(serde_json::to_string(v).unwrap_or_default()),
            )
        }
        ToolResultView::Generic(_) => ("completed".to_string(), None, result_text(data)),
        ToolResultView::Other => ("completed".to_string(), None, result_text(data)),
    }
}

fn result_outcome(data: &ToolResultData) -> String {
    if data.error.is_some() {
        "failed".to_string()
    } else {
        "completed".to_string()
    }
}

/// Model-facing result text: concatenate text blocks of the tool-result message.
fn result_text(data: &ToolResultData) -> Option<String> {
    let texts: Vec<&str> = data
        .message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

/// Render a `read` result view (`{ path, lines: [{ number, text }] }`) as
/// numbered file content. Returns None when the shape is not recognized.
fn render_read_view(view: &Value) -> Option<String> {
    let path = view.get("path").and_then(Value::as_str);
    let lines = view.get("lines").and_then(Value::as_array)?;
    let mut out = String::new();
    if let Some(path) = path {
        out.push_str(path);
        out.push('\n');
    }
    let width = lines
        .last()
        .and_then(|line| line.get("number"))
        .and_then(Value::as_u64)
        .map(|n| n.to_string().len())
        .unwrap_or(1);
    for line in lines {
        let number = line.get("number").and_then(Value::as_u64).unwrap_or(0);
        let text = line.get("text").and_then(Value::as_str).unwrap_or_default();
        out.push_str(&format!("{number:>width$} | {text}\n", width = width));
    }
    if out.is_empty() { None } else { Some(out) }
}

fn usage_event(usage: &TokenUsage) -> ClientEvent {
    ClientEvent::UsageUpdated {
        usage: UsageEvent {
            scope: workspace_model::UsageEventScope::TurnDelta,
            model: None,
            provider: None,
            agent_cli: None,
            tokens: workspace_model::UsageTokenBreakdown {
                input_tokens: Some(usage.input_tokens),
                output_tokens: Some(usage.output_tokens),
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                reasoning_tokens: usage.reasoning_tokens,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

/// `session/projection` `contextPressure` value — the harness's real context
/// occupancy. `projectedTokens` already prices in surface movement (and reacts
/// to compaction immediately), so prefer it over the bare `pressureTokens`
/// sample. Returns None when the projection carries no usable figure yet.
#[derive(serde::Deserialize)]
struct ContextPressureProjection {
    #[serde(default, rename = "pressureTokens")]
    pressure_tokens: Option<u64>,
    #[serde(default, rename = "projectedTokens")]
    projected_tokens: Option<u64>,
    #[serde(default, rename = "contextWindow")]
    context_window: Option<u64>,
}

fn context_pressure_usage_event(value: &Value) -> Option<ClientEvent> {
    let pressure: ContextPressureProjection = serde_json::from_value(value.clone()).ok()?;
    let used_tokens = pressure.projected_tokens.or(pressure.pressure_tokens);
    // Only emit when at least one figure advanced; otherwise we'd overwrite a
    // known occupancy with empty values on a no-op projection tick.
    if used_tokens.is_none() && pressure.context_window.is_none() {
        return None;
    }
    Some(ClientEvent::UsageUpdated {
        usage: UsageEvent {
            scope: UsageEventScope::ContextSnapshot,
            context: workspace_model::UsageContextSnapshot {
                used_tokens,
                window_tokens: pressure.context_window,
                ..Default::default()
            },
            ..Default::default()
        },
    })
}

/// `session/projection` `tokenUsage` value — durable cumulative provider usage
/// for the whole session. Emitted as `SessionTotal` so the reducer replaces the
/// per-turn-delta estimate with the harness's authoritative cumulative.
#[derive(serde::Deserialize)]
struct TokenUsageProjection {
    #[serde(default, rename = "uncachedInputTokens")]
    uncached_input_tokens: u64,
    #[serde(default, rename = "outputTokens")]
    output_tokens: u64,
    #[serde(default, rename = "cacheReadTokens")]
    cache_read_tokens: u64,
    #[serde(default, rename = "cacheWriteTokens")]
    cache_write_tokens: u64,
}

fn token_usage_projection_event(value: &Value) -> Option<ClientEvent> {
    let usage: TokenUsageProjection = serde_json::from_value(value.clone()).ok()?;
    Some(ClientEvent::UsageUpdated {
        usage: UsageEvent {
            scope: UsageEventScope::SessionTotal,
            tokens: UsageTokenBreakdown {
                input_tokens: Some(usage.uncached_input_tokens),
                output_tokens: Some(usage.output_tokens),
                cache_read_tokens: Some(usage.cache_read_tokens),
                cache_write_tokens: Some(usage.cache_write_tokens),
                ..Default::default()
            },
            ..Default::default()
        },
    })
}

/// Reconstruct a diff hunk list from a `FileDiff` (for `ToolDiffPreview`).
/// Used by history replay when the frontend wants hunk-level preview.
pub fn file_diff_to_hunks(path: &str, old: Option<&str>, new: &str) -> DiffHunk {
    let heading = path.to_string();
    let mut lines = Vec::new();
    if let Some(old) = old {
        for line in old.lines() {
            lines.push(DiffLine {
                kind: DiffLineKind::Context,
                content: line.to_string(),
            });
        }
    }
    for line in new.lines() {
        lines.push(DiffLine {
            kind: DiffLineKind::Added,
            content: line.to_string(),
        });
    }
    DiffHunk { heading, lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SessionSink;
    use acp_core::PermissionBroker;
    use std::sync::mpsc;

    fn test_sink() -> (SessionSink, mpsc::Receiver<ClientEvent>) {
        let (tx, rx) = mpsc::channel();
        (SessionSink::new(tx, PermissionBroker::default()), rx)
    }

    fn mux(json: serde_json::Value) -> MuxFrame {
        serde_json::from_value(json).expect("fixture frame must deserialize")
    }

    fn session_event(type_tag: &str, seq: u64, data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": { "type": type_tag, "seq": seq, "time": 0.0, "data": data }
        })
    }

    #[test]
    fn maps_request_header_does_not_emit_session_config() {
        // `request/header` must not emit an empty `SessionConfigUpdated`,
        // which would wipe the model control published by `emit_model_control`.
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "request/header",
            12,
            serde_json::json!({ "header": {}, "reason": "initial" }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(
            mapped.events.is_empty(),
            "request/header must not emit events: {:?}",
            mapped.events
        );
    }

    #[test]
    fn maps_assistant_chunk_text() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "assistant/chunk",
            1,
            serde_json::json!({ "turn": 1, "step": 1, "chunk": { "type": "text-delta", "index": 0, "text": "Hello" } }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::MessageChunk {
                role: MessageRole::Assistant,
                content: "Hello".to_string(),
            }]
        );
        assert_eq!(sink.last_seq.load(std::sync::atomic::Ordering::Acquire), 1);
    }

    #[test]
    fn maps_assistant_reasoning_chunk_to_thinking() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "assistant/chunk",
            2,
            serde_json::json!({ "turn": 1, "step": 1, "chunk": { "type": "reasoning-delta", "index": 0, "text": "think..." } }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![
                ClientEvent::ThinkingActivity { active: true },
                ClientEvent::ThinkingChunk {
                    text: "think...".to_string(),
                },
            ]
        );
    }

    #[test]
    fn live_assistant_message_emits_no_text() {
        // Live stream: text already arrived via assistant/chunk text-deltas;
        // the finalized assistant/message must not re-emit it (only usage).
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "assistant/message",
            3,
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "already streamed" }]
                },
                "usage": { "inputTokens": 10, "outputTokens": 5 }
            }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(
            mapped
                .events
                .iter()
                .all(|e| !matches!(e, ClientEvent::MessageChunk { .. })),
            "live assistant/message must not emit text: {:?}",
            mapped.events
        );
        assert!(
            mapped
                .events
                .iter()
                .any(|e| matches!(e, ClientEvent::UsageUpdated { .. })),
            "usage rollup must still be emitted"
        );
    }

    #[test]
    fn replay_assistant_message_emits_text() {
        // History replay (resume / re-baseline): no live chunks exist, so the
        // finalized assistant/message is the only text source and must emit.
        let (sink, _rx) = test_sink();
        sink.set_replaying(true);
        let frame = mux(session_event(
            "assistant/message",
            4,
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "from history" }]
                }
            }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::MessageChunk {
                role: MessageRole::Assistant,
                content: "from history".to_string(),
            }]
        );
        sink.set_replaying(false);
    }

    #[test]
    fn maps_tool_call_with_terminal_view() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/call",
                "seq": 3,
                "time": 0.0,
                "data": { "turn": 1, "step": 1, "callId": "call-1", "name": "bash", "arguments": "{\"command\":\"ls\"}" }
            },
            "view": { "for": "call", "view": { "card": "terminal", "title": "ls", "cwd": "/tmp" } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolStarted {
                id, name, kind, summary, raw_input, ..
            } if id == "call-1" && name == "bash" && kind == "execute" && summary == "ls"
                && raw_input.as_deref() == Some("{\"command\":\"ls\"}")
        ));
    }

    #[test]
    fn generic_card_view_tool_is_classified_as_read() {
        // dsh renders `view` (file read) with the generic card, which carries
        // no `kind`. Without inference the UI routes it to the Shell surface.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/call",
                "seq": 3,
                "time": 0.0,
                "data": { "turn": 1, "step": 1, "callId": "call-v", "name": "view", "arguments": "{\"path\":\"/a/b.rs\"}" }
            },
            "view": { "for": "call", "view": { "card": "generic", "title": "view /a/b.rs" } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolStarted { id, kind, summary, .. }
                if id == "call-v" && kind == "read" && summary == "view /a/b.rs"
        ));
    }

    #[test]
    fn generic_card_str_replace_tool_is_classified_as_edit() {
        // `str_replace` edits must land on the edit/diff surface so the
        // changes panel picks up the verified file change.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/call",
                "seq": 3,
                "time": 0.0,
                "data": { "turn": 1, "step": 1, "callId": "call-e", "name": "str_replace", "arguments": "{\"path\":\"/a/b.rs\",\"old_string\":\"x\",\"new_string\":\"y\"}" }
            },
            "view": { "for": "call", "view": { "card": "generic", "title": "str_replace /a/b.rs" } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolStarted { id, kind, .. } if id == "call-e" && kind == "edit"
        ));
    }

    #[test]
    fn missing_view_still_infers_kind_from_tool_name() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/call",
                "seq": 3,
                "time": 0.0,
                "data": { "turn": 1, "step": 1, "callId": "call-r", "name": "read_file", "arguments": "{\"path\":\"/a/b.rs\"}" }
            }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolStarted { id, kind, .. } if id == "call-r" && kind == "read"
        ));
    }

    #[test]
    fn maps_tool_result_diff_view_to_tool_diff_plus_completed() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/result",
                "seq": 4,
                "time": 0.0,
                "data": {
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "user",
                        "content": [{ "type": "tool-result", "toolCallId": "call-1", "content": [] }]
                    }
                }
            },
            "view": {
                "for": "result",
                "view": {
                    "card": "diff",
                    "diffs": [{ "path": "a.txt", "oldText": "old", "newText": "new" }]
                }
            }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![
                ClientEvent::ToolDiff {
                    id: "call-1".to_string(),
                    path: "a.txt".to_string(),
                    old_text: Some("old".to_string()),
                    new_text: "new".to_string(),
                },
                ClientEvent::ToolCompleted {
                    id: "call-1".to_string(),
                    name: None,
                    outcome: "completed".to_string(),
                    raw_output: None,
                    terminal_output: None,
                },
            ]
        );
    }

    #[test]
    fn maps_tool_result_terminal_view_to_terminal_output() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/result",
                "seq": 5,
                "time": 0.0,
                "data": {
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "user",
                        "content": [{ "type": "tool-result", "toolCallId": "call-2", "content": [] }]
                    }
                }
            },
            "view": { "for": "result", "view": { "card": "terminal", "output": "out", "exitCode": 0 } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolCompleted {
                terminal_output: Some(TerminalOutput { output, exit_code: Some(0), .. }),
                ..
            } if output == "out"
        ));
    }

    #[test]
    fn maps_todo_write_to_plan() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "todo/write",
            6,
            serde_json::json!({
                "todos": [
                    { "content": "Read code", "status": "in_progress" },
                    { "content": "Fix bug", "status": "pending" },
                    { "content": "Test", "status": "completed" },
                ]
            }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::PlanUpdated { entries } if entries.len() == 3
                && entries[0].status == AgentPlanEntryStatus::InProgress
                && entries[1].status == AgentPlanEntryStatus::Pending
                && entries[2].status == AgentPlanEntryStatus::Completed
        ));
    }

    #[test]
    fn maps_turn_end_to_finished() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "turn/end",
            7,
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::TurnFinished {
                stop_reason: "end_turn".to_string(),
                detail: None,
            }]
        );
    }

    #[test]
    fn maps_turn_end_error_to_refusal() {
        // dsh reports upstream LLM failures as `kind: "error"`; the bridge
        // maps it to `refusal` and carries the real `LlmFailure` message as
        // detail so the UI notice can name the actual upstream cause instead
        // of only the generic refusal wording.
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "turn/end",
            8,
            serde_json::json!({ "turn": 1, "reason": { "kind": "error", "error": { "message": "rate limited", "code": "RATE_LIMIT", "status": 429 } } }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::TurnFinished {
                stop_reason: "refusal".to_string(),
                detail: Some("HTTP 429 (RATE_LIMIT): rate limited".to_string()),
            }]
        );
    }

    #[test]
    fn maps_turn_end_error_detail_without_status() {
        // A failure payload carrying only a message still produces a usable
        // detail string (no HTTP status / code prefix).
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "turn/end",
            9,
            serde_json::json!({ "turn": 1, "reason": { "kind": "error", "error": { "message": "boom" } } }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::TurnFinished {
                stop_reason: "refusal".to_string(),
                detail: Some("boom".to_string()),
            }]
        );
    }

    #[test]
    fn maps_approval_requested_and_resolved() {
        let (sink, rx) = test_sink();
        let requested = mux(serde_json::json!({
            "type": "approval/requested",
            "sessionId": "s-1",
            "approvalId": "a-1",
            "toolName": "bash",
            "callId": "call-1",
            "reason": "shell"
        }));
        let mapped = map_mux_frame(&requested, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolPermissionRequest {
                id, name, options, details, input: None, ..
            } if id == "a-1" && name == "bash" && options.len() == 2 && details.as_deref() == Some("shell")
        ));

        let resolved = mux(serde_json::json!({
            "type": "approval/resolved",
            "sessionId": "s-1",
            "approvalId": "a-1",
            "outcome": "allowed-once"
        }));
        let mapped = map_mux_frame(&resolved, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::ToolPermissionResolved {
                id: "a-1".to_string(),
                outcome: "allowed-once".to_string(),
            }]
        );
        drop(rx);
    }

    #[test]
    fn maps_question_requested_to_input_request() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "question/requested",
            "sessionId": "s-1",
            "questions": [
                { "id": "q1", "question": "Proceed?", "options": [{ "label": "Yes" }, { "label": "No" }] }
            ]
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolPermissionRequest {
                id, name, options, input: Some(PermissionInputRequest { questions }), ..
            } if id == "q1" && name == "user_question" && questions.len() == 1
                && questions[0].options.len() == 2
                // The workbench's question panel enables 提交回答 only when it
                // finds a `submit` option; without one the form is unsubmittable.
                && options.iter().any(|option| option.id == "submit")
                && options.iter().any(|option| option.id == "cancel")
        ));
    }

    #[test]
    fn maps_session_projection_title() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/projection",
            "sessionId": "s-1",
            "key": "title",
            "value": "Fix auth bug",
            "seq": 8
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::SessionTitleUpdated {
                title: "Fix auth bug".to_string(),
            }]
        );
    }

    #[test]
    fn maps_context_pressure_projection_to_context_snapshot() {
        // The harness's real context occupancy rides the `contextPressure`
        // projection. `projectedTokens` (not the bare pressure sample) is the
        // numerator, and `contextWindow` the denominator — feeding both lets
        // the UI bar track the harness instead of the cumulative estimate.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/projection",
            "sessionId": "s-1",
            "key": "contextPressure",
            "value": {
                "pressureTokens": 12000,
                "projectedTokens": 12500,
                "contextWindow": 1000000
            },
            "seq": 9
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(mapped.events.len(), 1);
        match &mapped.events[0] {
            ClientEvent::UsageUpdated { usage } => {
                assert_eq!(usage.scope, UsageEventScope::ContextSnapshot);
                assert_eq!(usage.context.used_tokens, Some(12_500));
                assert_eq!(usage.context.window_tokens, Some(1_000_000));
            }
            other => panic!("expected UsageUpdated, got {other:?}"),
        }
    }

    #[test]
    fn maps_context_pressure_projection_prefers_projected_tokens() {
        // When only the bare pressure sample is present (no projection yet),
        // fall back to it so the bar still renders.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/projection",
            "sessionId": "s-1",
            "key": "contextPressure",
            "value": { "pressureTokens": 8000, "contextWindow": 200000 },
            "seq": 10
        }));
        let mapped = map_mux_frame(&frame, &sink);
        match &mapped.events[0] {
            ClientEvent::UsageUpdated { usage } => {
                assert_eq!(usage.context.used_tokens, Some(8_000));
                assert_eq!(usage.context.window_tokens, Some(200_000));
            }
            other => panic!("expected UsageUpdated, got {other:?}"),
        }
    }

    #[test]
    fn maps_context_pressure_projection_empty_is_noop() {
        // A projection tick with no figures must not wipe a known occupancy.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/projection",
            "sessionId": "s-1",
            "key": "contextPressure",
            "value": {},
            "seq": 11
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(mapped.events.is_empty(), "empty projection must not emit");
    }

    #[test]
    fn maps_token_usage_projection_to_session_total() {
        // The durable cumulative `tokenUsage` projection replaces the
        // per-turn-delta estimate with the harness's authoritative total.
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/projection",
            "sessionId": "s-1",
            "key": "tokenUsage",
            "value": {
                "uncachedInputTokens": 1000,
                "outputTokens": 500,
                "cacheReadTokens": 200,
                "cacheWriteTokens": 50
            },
            "seq": 12
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(mapped.events.len(), 1);
        match &mapped.events[0] {
            ClientEvent::UsageUpdated { usage } => {
                assert_eq!(usage.scope, UsageEventScope::SessionTotal);
                assert_eq!(usage.tokens.input_tokens, Some(1_000));
                assert_eq!(usage.tokens.output_tokens, Some(500));
                assert_eq!(usage.tokens.cache_read_tokens, Some(200));
                assert_eq!(usage.tokens.cache_write_tokens, Some(50));
            }
            other => panic!("expected UsageUpdated, got {other:?}"),
        }
    }

    #[test]
    fn maps_compaction_start_to_context_compaction_started() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "compaction/start",
            20,
            serde_json::json!({ "compactionId": "cmp-1", "turn": null }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert_eq!(mapped.events.len(), 1);
        match &mapped.events[0] {
            ClientEvent::ContextCompactionStarted { message } => {
                assert!(message.contains("正在压缩上下文"), "message={message}");
                assert!(message.contains("cmp-1"), "message should name compaction id: {message}");
            }
            other => panic!("expected ContextCompactionStarted, got {other:?}"),
        }
    }

    #[test]
    fn maps_compaction_end_to_context_compacted() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "compaction/end",
            21,
            serde_json::json!({ "compactionId": "cmp-1", "turn": null }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        match &mapped.events[0] {
            ClientEvent::ContextCompacted { message } => {
                assert_eq!(message, "上下文已压缩");
            }
            other => panic!("expected ContextCompacted, got {other:?}"),
        }
    }

    #[test]
    fn maps_compaction_end_with_error_surfaces_it() {
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "compaction/end",
            22,
            serde_json::json!({ "compactionId": "cmp-1", "turn": null, "error": "summarize failed" }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        match &mapped.events[0] {
            ClientEvent::ContextCompacted { message } => {
                assert!(message.contains("未完成"), "message={message}");
                assert!(message.contains("summarize failed"), "message={message}");
            }
            other => panic!("expected ContextCompacted, got {other:?}"),
        }
    }

    #[test]
    fn maps_host_agent_error_to_interrupted() {
        let frame: HostFrame = serde_json::from_value(serde_json::json!({
            "type": "host/agent-error",
            "sessionId": "s-1",
            "message": "boom"
        }))
        .unwrap();
        let mapped = map_host_frame(&frame);
        assert_eq!(
            mapped.events,
            vec![ClientEvent::Interrupted {
                reason: "harness agent error: boom".to_string(),
            }]
        );
    }

    #[test]
    fn unknown_event_type_is_ignored_not_fatal() {
        // Additive harness schema change: a new event type we do not know must
        // not produce events or break the stream (12.7).
        let (sink, _rx) = test_sink();
        let frame = mux(session_event(
            "session/future-event",
            9,
            serde_json::json!({ "anything": true }),
        ));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(mapped.events.is_empty());
        // last_seq still advances so re-baseline resumes past the unknown frame.
        assert_eq!(sink.last_seq.load(std::sync::atomic::Ordering::Acquire), 9);
    }

    #[test]
    fn unknown_tool_card_falls_back_to_generic() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/call",
                "seq": 10,
                "time": 0.0,
                "data": { "turn": 1, "step": 1, "callId": "call-9", "name": "future-tool", "arguments": "{}" }
            },
            "view": { "for": "call", "view": { "card": "future-card", "title": "x" } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolStarted { id, .. } if id == "call-9"
        ));
    }

    #[test]
    fn read_result_view_renders_numbered_file_content() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/result",
                "seq": 11,
                "time": 0.0,
                "data": {
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "user",
                        "content": [{ "type": "tool-result", "toolCallId": "call-10", "content": [] }]
                    }
                }
            },
            "view": { "for": "result", "view": { "card": "read", "path": "a.rs", "lines": [{ "number": 1, "text": "fn main() {}" }] } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        match &mapped.events[0] {
            ClientEvent::ToolCompleted { raw_output: Some(raw), .. } => {
                assert!(raw.contains("a.rs"), "path header missing: {raw}");
                assert!(raw.contains("1 | fn main() {}"), "numbered line missing: {raw}");
                assert!(!raw.contains("\"lines\""), "must not be raw JSON: {raw}");
            }
            other => panic!("expected ToolCompleted with raw_output, got {other:?}"),
        }
    }

    #[test]
    fn unrepresentable_view_serializes_to_raw_output_json() {
        let (sink, _rx) = test_sink();
        let frame = mux(serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "tool/result",
                "seq": 11,
                "time": 0.0,
                "data": {
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "user",
                        "content": [{ "type": "tool-result", "toolCallId": "call-11", "content": [] }]
                    }
                }
            },
            "view": { "for": "result", "view": { "card": "search", "query": "foo", "hits": [] } }
        }));
        let mapped = map_mux_frame(&frame, &sink);
        assert!(matches!(
            &mapped.events[0],
            ClientEvent::ToolCompleted { raw_output: Some(raw), .. } if raw.contains("foo")
        ));
    }

    #[test]
    fn history_replay_reconstructs_client_event_sequence() {
        // 8.3: a fixture history page (assistant text + tool call + turn end)
        // reconstructs the expected ClientEvent sequence.
        let (sink, _rx) = test_sink();
        sink.set_replaying(true);
        let history = serde_json::json!([
            session_event(
                "assistant/message",
                1,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "assistant",
                        "content": [{ "type": "text", "text": "Working on it" }]
                    }
                })
            ),
            session_event(
                "tool/call",
                2,
                serde_json::json!({
                    "turn": 1, "step": 1, "callId": "call-1", "name": "bash", "arguments": "{\"command\":\"ls\"}"
                })
            ),
            session_event(
                "tool/result",
                3,
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "user",
                        "content": [{ "type": "tool-result", "toolCallId": "call-1", "content": [{ "type": "text", "text": "done" }] }]
                    }
                })
            ),
            session_event(
                "turn/end",
                4,
                serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } })
            ),
        ]);
        let frames: Vec<MuxFrame> = history
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(mux)
            .collect();
        let mut events = Vec::new();
        for frame in &frames {
            events.extend(map_mux_frame(frame, &sink).events);
        }
        // assistant message → tool started → tool completed → turn finished
        assert!(
            matches!(&events[0], ClientEvent::MessageChunk { role: MessageRole::Assistant, content } if content == "Working on it")
        );
        assert!(matches!(&events[1], ClientEvent::ToolStarted { id, .. } if id == "call-1"));
        assert!(matches!(&events[2], ClientEvent::ToolCompleted { id, .. } if id == "call-1"));
        assert!(matches!(&events[3], ClientEvent::TurnFinished { .. }));
        // last_seq ends at the final event so a re-baseline resumes past it.
        assert_eq!(sink.last_seq.load(std::sync::atomic::Ordering::Acquire), 4);
    }
}
