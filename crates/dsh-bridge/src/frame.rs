//! Lenient frame unions: `MuxFrame`, `HostFrame`, and the embedded
//! `SessionEvent` / `ToolEventView` mirrors.
//!
//! Mirrors `deepseek-harness/packages/host/apiproxy/src/api/events.schema.ts`
//! and `.../sessions.schema.ts`. Unknown variants fall back to a generic
//! `Other` arm (carrying the raw JSON) so an additive harness schema change
//! never breaks the stream — the design doc's lenient-deserialization decision.

use serde::Deserialize;
use serde_json::Value;

use crate::rpc_types::{ApprovalRequestId, RpcId, SessionId};

/// One `SessionEvent` — strict envelope (`type`/`seq`/`time`) + wide `data`.
/// `ignorable` marks an event a reader may skip when it does not recognize the
/// type (the dsh merge-extensibility guard).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionEvent {
    #[serde(rename = "type")]
    pub type_tag: String,
    pub seq: u64,
    pub time: f64,
    pub data: Value,
    #[serde(default, rename = "sourceEventSeqs")]
    pub source_event_seqs: Option<Vec<u64>>,
    #[serde(default, rename = "surfaceOp")]
    pub surface_op: Option<Value>,
    #[serde(default)]
    pub ignorable: Option<bool>,
}

impl SessionEvent {
    pub fn data<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.data.clone()).ok()
    }
}

/// `ToolEventView` — `{ for: "call"|"result", view: { card, ... } }`. The view
/// interior is held as opaque JSON and narrowed per `card` in the mapping layer
/// (mirrors dsh's own `toolEventViewSchema` which locks only the `for`
/// discriminant + presence of `view.card`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "for")]
pub enum ToolEventView {
    #[serde(rename = "call")]
    Call { view: ToolCallView },
    #[serde(rename = "result")]
    Result { view: ToolResultView },
}

/// `ToolCallView` — a `card`-tagged union with a fallback for unknown cards.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "card")]
pub enum ToolCallView {
    #[serde(rename = "generic")]
    Generic(GenericCallView),
    #[serde(rename = "terminal")]
    Terminal(TerminalCallView),
    #[serde(rename = "diff")]
    Diff(DiffCallView),
    #[serde(other)]
    Other,
}

impl ToolCallView {
    pub fn card(&self) -> &'static str {
        match self {
            Self::Generic(_) => "generic",
            Self::Terminal(_) => "terminal",
            Self::Diff(_) => "diff",
            Self::Other => "other",
        }
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Self::Generic(v) => Some(&v.title),
            Self::Terminal(v) => Some(&v.title),
            Self::Diff(v) => Some(&v.title),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenericCallView {
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, rename = "rawInput")]
    pub raw_input: Option<Value>,
    #[serde(default)]
    pub content: Option<Vec<Value>>,
    #[serde(default)]
    pub locations: Option<Vec<FileLocation>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalCallView {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiffCallView {
    pub title: String,
    pub diffs: Vec<FileDiff>,
    #[serde(default)]
    pub locations: Option<Vec<FileLocation>>,
}

/// `ToolResultView` — a `card`-tagged union with a fallback for unknown cards.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "card")]
pub enum ToolResultView {
    #[serde(rename = "generic")]
    Generic(GenericResultView),
    #[serde(rename = "terminal")]
    Terminal(TerminalResultView),
    #[serde(rename = "diff")]
    Diff(DiffResultView),
    #[serde(rename = "search")]
    Search(Value),
    #[serde(rename = "read")]
    Read(Value),
    #[serde(rename = "web")]
    Web(Value),
    #[serde(other)]
    Other,
}

impl ToolResultView {
    pub fn card(&self) -> &'static str {
        match self {
            Self::Generic(_) => "generic",
            Self::Terminal(_) => "terminal",
            Self::Diff(_) => "diff",
            Self::Search(_) => "search",
            Self::Read(_) => "read",
            Self::Web(_) => "web",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenericResultView {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalResultView {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default, rename = "exitCode")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiffResultView {
    #[serde(default)]
    pub title: Option<String>,
    pub diffs: Vec<FileDiff>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDiff {
    pub path: String,
    #[serde(default, rename = "oldText")]
    pub old_text: Option<String>,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileLocation {
    pub path: String,
    #[serde(default)]
    pub line: Option<u32>,
}

/// `MuxFrame` union — the payload slot of an `events.mux` `ServerRequest`.
/// Unknown `type` variants fall back to `Other` carrying the raw JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum MuxFrame {
    #[serde(rename = "session/event")]
    SessionEvent {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        event: SessionEvent,
        #[serde(default)]
        view: Option<ToolEventView>,
    },
    #[serde(rename = "session/subscribed")]
    SessionSubscribed {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(rename = "lastSeq")]
        last_seq: i64,
    },
    #[serde(rename = "approval/requested")]
    ApprovalRequested {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(rename = "approvalId")]
        approval_id: ApprovalRequestId,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default, rename = "callId")]
        call_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(rename = "approval/resolved")]
    ApprovalResolved {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(rename = "approvalId")]
        approval_id: ApprovalRequestId,
        outcome: String,
    },
    #[serde(rename = "question/requested")]
    QuestionRequested {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        questions: Vec<AskUserQuestionItem>,
    },
    #[serde(rename = "question/resolved")]
    QuestionResolved {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(rename = "questionRpcId")]
        question_rpc_id: RpcId,
        outcome: String,
    },
    #[serde(rename = "session/queue")]
    SessionQueue {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(default)]
        items: Vec<Value>,
    },
    #[serde(rename = "session/jobs")]
    SessionJobs {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(default)]
        jobs: Vec<Value>,
    },
    #[serde(rename = "session/projection")]
    SessionProjection {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        key: String,
        value: Value,
        seq: u64,
    },
    #[serde(rename = "stream/error")]
    StreamError { error: crate::rpc_types::RpcError },
    #[serde(other)]
    Other,
}

impl MuxFrame {
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            MuxFrame::SessionEvent { session_id, .. }
            | MuxFrame::SessionSubscribed { session_id, .. }
            | MuxFrame::ApprovalRequested { session_id, .. }
            | MuxFrame::ApprovalResolved { session_id, .. }
            | MuxFrame::QuestionRequested { session_id, .. }
            | MuxFrame::QuestionResolved { session_id, .. }
            | MuxFrame::SessionQueue { session_id, .. }
            | MuxFrame::SessionJobs { session_id, .. }
            | MuxFrame::SessionProjection { session_id, .. } => Some(session_id),
            MuxFrame::StreamError { .. } | MuxFrame::Other => None,
        }
    }
}

/// `HostFrame` union — the payload slot of an `events.host` `ServerRequest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum HostFrame {
    #[serde(rename = "host/session-added")]
    HostSessionAdded {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        #[serde(default)]
        blank: bool,
        #[serde(default, rename = "parentSessionId")]
        parent_session_id: Option<SessionId>,
        #[serde(default)]
        origin: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default, rename = "agentPreset")]
        agent_preset: Option<String>,
    },
    #[serde(rename = "host/session-removed")]
    HostSessionRemoved {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
    },
    #[serde(rename = "host/session-status")]
    HostSessionStatus {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        running: bool,
    },
    #[serde(rename = "host/agent-error")]
    HostAgentError {
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        message: String,
    },
    #[serde(rename = "host/workspace-changed")]
    HostWorkspaceChanged { workspace: Value },
    #[serde(rename = "host/workspace-removed")]
    HostWorkspaceRemoved { workspace_id: String },
    #[serde(rename = "host/workspace-order-changed")]
    HostWorkspaceOrderChanged { workspace_ids: Vec<String> },
    #[serde(rename = "host/archived-sessions-changed")]
    HostArchivedSessionsChanged {
        archived_session_ids: Vec<SessionId>,
    },
    #[serde(rename = "host/remote-event")]
    HostRemoteEvent { event: String, args: Vec<Value> },
    #[serde(rename = "stream/error")]
    StreamError { error: crate::rpc_types::RpcError },
    #[serde(other)]
    Other,
}

impl HostFrame {
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            HostFrame::HostSessionAdded { session_id, .. }
            | HostFrame::HostSessionRemoved { session_id }
            | HostFrame::HostSessionStatus { session_id, .. }
            | HostFrame::HostAgentError { session_id, .. } => Some(session_id),
            HostFrame::HostWorkspaceChanged { .. }
            | HostFrame::HostWorkspaceRemoved { .. }
            | HostFrame::HostWorkspaceOrderChanged { .. }
            | HostFrame::HostArchivedSessionsChanged { .. }
            | HostFrame::HostRemoteEvent { .. }
            | HostFrame::StreamError { .. }
            | HostFrame::Other => None,
        }
    }
}

/// One user-question item (the dsh `AskUserQuestionItem`).
#[derive(Debug, Clone, Deserialize)]
pub struct AskUserQuestionItem {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<AskUserQuestionOption>>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: Option<bool>,
    #[serde(default)]
    pub intent: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AskUserQuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// `TodoItem` from a `todo/write` event.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
}

/// `turn/end` reason — `{ kind, ... }`. The rest is held as opaque JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnEndReason {
    pub kind: String,
    #[serde(flatten)]
    pub rest: Value,
}

/// `assistant/chunk` data — `{ turn, step, chunk: StreamChunk }`.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantChunkData {
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub step: u64,
    pub chunk: StreamChunk,
}

/// `StreamChunk` union (text-delta / reasoning-delta / tool-call-delta / ...).
/// Only the consumed variants are typed; the rest fall through to `Other`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum StreamChunk {
    #[serde(rename = "text-delta")]
    TextDelta { index: u64, text: String },
    #[serde(rename = "reasoning-delta")]
    ReasoningDelta { index: u64, text: String },
    #[serde(rename = "tool-call-delta")]
    ToolCallDelta {
        index: u64,
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(rename = "argumentsDelta")]
        arguments_delta: String,
    },
    #[serde(rename = "usage")]
    Usage { usage: TokenUsage },
    #[serde(rename = "finish")]
    Finish { reason: Value },
    #[serde(other)]
    Other,
}

/// `assistant/message` data — `{ turn, step, message, usage? }`.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessageData {
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub step: u64,
    pub message: AssistantMessage,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

/// `AssistantMessage` — `{ id, role, content: [ContentBlock], source }`. Only
/// text/reasoning blocks are narrowed; the rest stay opaque.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    #[serde(default)]
    pub id: Option<String>,
    pub role: String,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub source: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    #[serde(rename = "tool-call")]
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        content: Vec<ContentBlock>,
        #[serde(default)]
        #[serde(rename = "isError")]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

/// `tool/call` data — `{ turn, step, callId, name, arguments }`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallData {
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub step: u64,
    #[serde(rename = "callId")]
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// `tool/result` data — `{ turn, step, message, error?, meta? }`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolResultData {
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub step: u64,
    pub message: ToolResultMessage,
    #[serde(default)]
    pub error: Option<ToolResultError>,
    #[serde(default)]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolResultMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolResultError {
    pub name: String,
    pub code: String,
}

/// `TokenUsage` — `{ inputTokens, outputTokens, cacheReadTokens?, ... }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenUsage {
    #[serde(rename = "inputTokens", default)]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens", default)]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadTokens", default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(rename = "cacheWriteTokens", default)]
    pub cache_write_tokens: Option<u64>,
    #[serde(rename = "reasoningTokens", default)]
    pub reasoning_tokens: Option<u64>,
}

/// `request/header` data — `{ header, reason }`. Held as opaque JSON (the
/// mapping layer extracts only what `SessionConfigUpdated` needs).
#[derive(Debug, Clone, Deserialize)]
pub struct RequestHeaderData {
    pub header: Value,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_frame_session_event_parse() {
        let raw = serde_json::json!({
            "type": "session/event",
            "sessionId": "s-1",
            "event": {
                "type": "assistant/chunk",
                "seq": 5,
                "time": 1700000000.0,
                "data": { "turn": 1, "step": 1, "chunk": { "type": "text-delta", "index": 0, "text": "hi" } }
            }
        });
        let frame: MuxFrame = serde_json::from_value(raw).unwrap();
        match frame {
            MuxFrame::SessionEvent { event, .. } => {
                assert_eq!(event.type_tag, "assistant/chunk");
                let data: AssistantChunkData = event.data().unwrap();
                assert!(matches!(data.chunk, StreamChunk::TextDelta { text, .. } if text == "hi"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mux_frame_unknown_type_falls_back() {
        let raw = serde_json::json!({ "type": "session/future-event", "sessionId": "s-1" });
        let frame: MuxFrame = serde_json::from_value(raw).unwrap();
        assert!(matches!(frame, MuxFrame::Other));
    }

    #[test]
    fn tool_call_view_unknown_card_falls_back() {
        let raw =
            serde_json::json!({ "for": "call", "view": { "card": "future-card", "title": "x" } });
        let view: ToolEventView = serde_json::from_value(raw).unwrap();
        match view {
            ToolEventView::Call { view } => assert!(matches!(view, ToolCallView::Other)),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tool_call_view_terminal_parse() {
        let raw = serde_json::json!({ "for": "call", "view": { "card": "terminal", "title": "ls", "cwd": "/tmp" } });
        let view: ToolEventView = serde_json::from_value(raw).unwrap();
        match view {
            ToolEventView::Call { view } => assert!(matches!(view, ToolCallView::Terminal(_))),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tool_result_view_diff_parse() {
        let raw = serde_json::json!({
            "for": "result",
            "view": { "card": "diff", "diffs": [{ "path": "a.txt", "oldText": null, "newText": "hi" }] }
        });
        let view: ToolEventView = serde_json::from_value(raw).unwrap();
        match view {
            ToolEventView::Result { view } => match view {
                ToolResultView::Diff(d) => assert_eq!(d.diffs.len(), 1),
                _ => panic!("wrong result variant"),
            },
            _ => panic!("wrong for"),
        }
    }

    #[test]
    fn host_frame_session_status_parse() {
        let raw = serde_json::json!({ "type": "host/session-status", "sessionId": "s-1", "running": true });
        let frame: HostFrame = serde_json::from_value(raw).unwrap();
        match frame {
            HostFrame::HostSessionStatus { running, .. } => assert!(running),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn session_event_ignorable_default_none() {
        let raw = serde_json::json!({
            "type": "turn/start", "seq": 1, "time": 0.0, "data": { "turn": 1 }
        });
        let event: SessionEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(event.ignorable, None);
    }

    #[test]
    fn approval_requested_parse() {
        let raw = serde_json::json!({
            "type": "approval/requested",
            "sessionId": "s-1",
            "approvalId": "a-1",
            "toolName": "bash",
            "callId": "c-1",
            "reason": "shell"
        });
        let frame: MuxFrame = serde_json::from_value(raw).unwrap();
        match frame {
            MuxFrame::ApprovalRequested { tool_name, .. } => assert_eq!(tool_name, "bash"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn question_requested_parse() {
        let raw = serde_json::json!({
            "type": "question/requested",
            "sessionId": "s-1",
            "questions": [{ "id": "q1", "question": "ok?" }]
        });
        let frame: MuxFrame = serde_json::from_value(raw).unwrap();
        match frame {
            MuxFrame::QuestionRequested { questions, .. } => assert_eq!(questions.len(), 1),
            _ => panic!("wrong variant"),
        }
    }
}
