//! Lenient serde mirrors of the dsh host RPC envelope and the control-method
//! payloads the bridge uses.
//!
//! The dsh schema is a private, versioned contract with no stability guarantee
//! (see `deepseek-harness/packages/host/apiproxy/src/api/rpc.schema.ts`).
//! Deserialization is lenient: unknown fields are ignored (`#[serde(default)]`
//! on optional fields, opaque [`serde_json::Value`] for variable payloads), so
//! additive schema changes do not break the stream. Only removals or shape
//! changes of consumed fields break, which integration tests pin.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// RPC correlation id (a UUID string on the wire; branded `RpcId` in dsh).
pub type RpcId = String;

/// dsh session id (a non-empty string; branded `SessionId` in dsh).
pub type SessionId = String;

/// dsh approval request id (a non-empty string).
pub type ApprovalRequestId = String;

/// `ClientRequest` full form — the body of `POST /api/<method>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRequest {
    /// Wire tag — always the literal `"client-request"`.
    #[serde(rename = "type")]
    pub type_tag: String,
    pub rpcId: RpcId,
    pub method: String,
    pub payload: Value,
}

impl ClientRequest {
    pub fn new(rpc_id: RpcId, method: impl Into<String>, payload: Value) -> Self {
        Self {
            type_tag: "client-request".to_string(),
            rpcId: rpc_id,
            method: method.into(),
            payload,
        }
    }
}

/// `ServerResponse` full form — the HTTP response body of a control POST.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerResponse {
    /// Wire tag — always the literal `"server-response"`.
    #[serde(rename = "type")]
    pub type_tag: String,
    pub rpcId: RpcId,
    pub result: RpcResult<Value>,
}

/// `ServerRequest` full form — one SSE frame (a server-initiated message).
/// `method` is the frame's `type` (e.g. `session/event`); `payload` is the
/// `MuxFrame`/`HostFrame` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerRequest {
    /// Wire tag — always the literal `"server-request"`.
    #[serde(rename = "type")]
    pub type_tag: String,
    pub rpcId: RpcId,
    pub method: String,
    pub payload: Value,
}

/// `ClientResponse` full form — the body of `POST /api/respond`, answering a
/// server-request (approval/question) by echoing its `rpcId`.
#[derive(Debug, Clone, Serialize)]
pub struct ClientResponse {
    /// Wire tag — always the literal `"client-response"`.
    #[serde(rename = "type")]
    pub type_tag: String,
    pub rpcId: RpcId,
    pub result: RpcResult<Value>,
}

impl ClientResponse {
    pub fn ok(rpc_id: RpcId, value: Value) -> Self {
        Self {
            type_tag: "client-response".to_string(),
            rpcId: rpc_id,
            result: RpcResult::Ok { ok: true, value },
        }
    }
}

/// Business success/failure result. The error arm is held as opaque JSON so an
/// unknown error `code` does not break deserialization.
///
/// The `Err` arm is tried FIRST: with `value` defaulted on `Ok`, an untagged
/// match ordered Ok-first would swallow error envelopes (`{"ok":false,
/// "error":…}` parses fine as `Ok` with the default `value`). Err-first keeps
/// error parsing authoritative while still accepting the typert void result
/// (`{"ok":true}` with no `value` field — e.g. `commands/execute` for an
/// unresolvable command line), which deserializes as `Ok` with `Value::Null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcResult<T> {
    Err { ok: bool, error: RpcError },
    Ok {
        ok: bool,
        #[serde(default)]
        value: T,
    },
}

impl<T> RpcResult<T> {
    pub fn is_ok(&self) -> bool {
        matches!(self, RpcResult::Ok { .. })
    }

    pub fn ok_value(&self) -> Option<&T> {
        match self {
            RpcResult::Ok { value, .. } => Some(value),
            RpcResult::Err { .. } => None,
        }
    }

    pub fn err(&self) -> Option<&RpcError> {
        match self {
            RpcResult::Ok { .. } => None,
            RpcResult::Err { error, .. } => Some(error),
        }
    }
}

/// dsh `RpcError` — `{ code, message, details }`. `details` is opaque; `code`
/// is a string so unknown codes deserialize without failing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Extract the dsh error `code` from a `call()` failure. `call()` formats
/// business errors via `RpcError`'s Display as `"{code}: {message}"`, so the
/// code is recoverable from the message prefix before the first `": "`.
pub fn rpc_error_code(err: &anyhow::Error) -> Option<String> {
    let msg = format!("{err}");
    msg.split(": ")
        .next()
        .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(|code| code.to_string())
}

/// `RpcReceipt` — the HTTP response body of `POST /api/respond`. A late or
/// duplicate respond yields `not-pending`; a malformed body yields `bad-response`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RpcReceipt {
    Accepted { accepted: bool },
    Rejected { accepted: bool, reason: String },
}

impl RpcReceipt {
    pub fn accepted(&self) -> bool {
        matches!(self, RpcReceipt::Accepted { accepted: true })
    }
}

// ---- Control-method payloads (the ones the bridge issues) ----
// These are `Serialize` (request) / `Deserialize` (response) mirrors. Optional
// fields use `#[serde(default)]` and `skip_serializing_if = "Option::is_none"`
// so absent fields stay absent on the wire (dsh schemas use
// exactOptionalPropertyTypes).

/// `session.create` request (`{ cwd?, workspaceId?, sessionId?, agentPreset? }`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionCreatePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "agentPreset")]
    pub agent_preset: Option<String>,
}

/// `session.create` response value.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionCreateValue {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    #[serde(default, rename = "agentPreset")]
    pub agent_preset: Option<String>,
}

/// `agentPreset.list` request payload (empty object).
#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentPresetListPayload {}

/// One entry of `agentPreset.list` (`{ id, trust, isDefault, name?, description?, broken? }`).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentPresetEntry {
    pub id: String,
    #[serde(default)]
    pub trust: Option<String>,
    #[serde(default, rename = "isDefault")]
    pub is_default: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `agentPreset.list` response value.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentPresetListValue {
    #[serde(default)]
    pub presets: Vec<AgentPresetEntry>,
}

/// `agentPreset.select` request payload (`{ sessionId, agentPreset }`).
#[derive(Debug, Clone, Serialize)]
pub struct AgentPresetSelectPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    #[serde(rename = "agentPreset")]
    pub agent_preset: String,
}

/// `agentPreset.select` response value.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentPresetSelectValue {
    #[serde(rename = "agentPreset")]
    pub agent_preset: String,
}

/// `session.prompt` request. `mode` is `queue` (a new turn) or `steer`
/// (steering input into an active turn).
#[derive(Debug, Clone, Serialize)]
pub struct SessionPromptPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    pub mode: PromptMode,
    pub content: Vec<PromptContentPart>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "clientTimeZone")]
    pub client_time_zone: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptMode {
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "steer")]
    Steer,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptContentPart {
    Text {
        text: String,
    },
    Image {
        #[serde(rename = "mediaType")]
        media_type: String,
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl PromptContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// `session.prompt` response value.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionPromptValue {
    pub accepted: bool,
}

/// `session.cancel` request.
#[derive(Debug, Clone, Serialize)]
pub struct SessionCancelPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
}

/// `session.cancel` response value.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionCancelValue {
    pub accepted: bool,
}

/// `session.history` request (`{ sessionId, beforeSeq?, maxMessages? }`).
#[derive(Debug, Clone, Serialize)]
pub struct SessionHistoryPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none", rename = "beforeSeq")]
    pub before_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxMessages")]
    pub max_messages: Option<u32>,
}

/// `session.history` response value. `events` is the page of `HistoryEntry`s
/// (each `{ event, view? }`); `has_more` indicates an older page exists.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionHistoryValue {
    #[serde(default)]
    pub events: Vec<HistoryEntryRaw>,
    #[serde(default)]
    pub has_more: bool,
}

/// One history entry, held as opaque JSON so the mapping layer can narrow it.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntryRaw {
    pub event: Value,
    #[serde(default)]
    pub view: Option<Value>,
}

/// `session.models` request.
#[derive(Debug, Clone, Serialize)]
pub struct SessionModelsPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
}

/// `session.selectModel` request.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSelectModelPayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
}

/// `session.list` request (`{ cursor? }`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionListPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `host.describe` request (empty object).
#[derive(Debug, Clone, Default, Serialize)]
pub struct HostDescribePayload {}

/// `host.describe` response value — used for the startup probe and version pin.
#[derive(Debug, Clone, Deserialize)]
pub struct HostDescribeValue {
    pub version: String,
    pub cwd: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(rename = "attachedSessions", default)]
    pub attached_sessions: u32,
    #[serde(rename = "canOpenPath", default)]
    pub can_open_path: bool,
}

/// `session.list` response value — used for the startup probe fallback.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionListValue {
    #[serde(default)]
    pub items: Vec<Value>,
}

/// `respond` payload for an approval answer.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResponsePayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    #[serde(rename = "approvalId")]
    pub approval_id: ApprovalRequestId,
    pub outcome: ApprovalOutcomeWire,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalOutcomeWire {
    #[serde(rename = "allowed-once")]
    AllowedOnce,
    #[serde(rename = "rejected")]
    Rejected,
}

/// `respond` payload for a question answer batch.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionResponsePayload {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,
    pub answer: AskUserQuestionAnswerWire,
}

#[derive(Debug, Clone, Serialize)]
pub struct AskUserQuestionAnswerWire {
    pub answers: Vec<AskUserQuestionAnswerItemWire>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AskUserQuestionAnswerItemWire {
    pub id: String,
    pub selected: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

// ---- `commands/execute` (typert Remote surface) ----
//
// Unlike the dotted legacy methods (`session.create`, ...), typert Remote
// endpoints live at `POST /api/<namespace>/<method>` and require the payload
// to be exactly `{ "args": { ...named wire fields... } }`. The gateway
// validates the args shape against the generated descriptor and rejects
// anything else with `arguments-invalid`.

/// `commands/execute` request payload: exactly one `args` object whose fields
/// are the descriptor's wire names (`agentId`, `line`, `images`).
#[derive(Debug, Clone, Serialize)]
pub struct CommandsExecutePayload {
    pub args: CommandsExecuteArgs,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandsExecuteArgs {
    /// Wire name for the descriptor's `agent` lookup parameter: the session id.
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// Full command line including the leading slash (e.g. `/compact`).
    pub line: String,
    /// Base64 image attachments; always empty for the commands kodex issues.
    pub images: Vec<serde_json::Value>,
}

/// `commands/execute` response value: the settled `CommandExecution`, present
/// only when the line resolved to a registered command. Absent (void) means
/// unknown or malformed command.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandsExecuteValue {
    #[serde(rename = "commandId")]
    pub command_id: String,
    pub result: CommandsExecuteResult,
}

/// One settled command outcome. `sourceEventSeq` rides only on success.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum CommandsExecuteResult {
    #[serde(rename = "success")]
    Success {
        #[serde(default)]
        text: Option<String>,
        #[serde(rename = "sourceEventSeq", default)]
        source_event_seq: Option<u64>,
    },
    #[serde(rename = "error")]
    Error { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_request_round_trip() {
        let req = ClientRequest::new(
            "rpc-1".into(),
            "session.create",
            serde_json::json!({ "cwd": "/tmp" }),
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "client-request");
        assert_eq!(json["rpcId"], "rpc-1");
        assert_eq!(json["method"], "session.create");
        assert_eq!(json["payload"]["cwd"], "/tmp");
    }

    #[test]
    fn server_response_ok_parse() {
        let raw = serde_json::json!({
            "type": "server-response",
            "rpcId": "rpc-1",
            "result": { "ok": true, "value": { "sessionId": "s-1" } },
        });
        let resp: ServerResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(resp.rpcId, "rpc-1");
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.ok_value().unwrap()["sessionId"], "s-1");
    }

    #[test]
    fn server_response_err_parse() {
        let raw = serde_json::json!({
            "type": "server-response",
            "rpcId": "rpc-1",
            "result": {
                "ok": false,
                "error": { "code": "session-not-found", "message": "nope", "details": { "sessionId": "s-1" } }
            },
        });
        let resp: ServerResponse = serde_json::from_value(raw).unwrap();
        let err = resp.result.err().unwrap();
        assert_eq!(err.code, "session-not-found");
        assert_eq!(err.message, "nope");
    }

    #[test]
    fn server_response_void_ok_parse() {
        // Typert void business result: no `value` field at all. Must parse as
        // Ok with `Value::Null` — and must not be mistaken for an error.
        let raw = serde_json::json!({
            "type": "server-response",
            "rpcId": "rpc-1",
            "result": { "ok": true },
        });
        let resp: ServerResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.ok_value().unwrap(), &serde_json::Value::Null);
    }

    #[test]
    fn commands_execute_value_parse() {
        let raw = serde_json::json!({
            "commandId": "cmd-1",
            "result": { "kind": "success", "text": "Compacted 3 history items (~1.2k tokens)." }
        });
        let value: crate::rpc_types::CommandsExecuteValue = serde_json::from_value(raw).unwrap();
        assert_eq!(value.command_id, "cmd-1");
        match value.result {
            crate::rpc_types::CommandsExecuteResult::Success { text, .. } => {
                assert!(text.unwrap().contains("Compacted"));
            }
            other => panic!("expected success, got {other:?}"),
        }

        let raw = serde_json::json!({
            "commandId": "cmd-2",
            "result": { "kind": "error", "text": "Compaction cancelled." }
        });
        let value: crate::rpc_types::CommandsExecuteValue = serde_json::from_value(raw).unwrap();
        match value.result {
            crate::rpc_types::CommandsExecuteResult::Error { text } => {
                assert_eq!(text, "Compaction cancelled.");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn commands_execute_payload_wire_shape() {
        // The typert gateway requires the payload to be exactly one `args`
        // object whose fields are the descriptor wire names.
        let payload = crate::rpc_types::CommandsExecutePayload {
            args: crate::rpc_types::CommandsExecuteArgs {
                agent_id: "s-1".into(),
                line: "/compact".into(),
                images: Vec::new(),
            },
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["args"]["agentId"], "s-1");
        assert_eq!(json["args"]["line"], "/compact");
        assert_eq!(json["args"]["images"], serde_json::json!([]));
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn server_request_parse() {
        let raw = serde_json::json!({
            "type": "server-request",
            "rpcId": "rpc-2",
            "method": "session/event",
            "payload": { "type": "session/subscribed", "sessionId": "s-1", "lastSeq": 3 },
        });
        let req: ServerRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.rpcId, "rpc-2");
        assert_eq!(req.method, "session/event");
        assert_eq!(req.payload["type"], "session/subscribed");
    }

    #[test]
    fn rpc_receipt_accepted() {
        let raw = serde_json::json!({ "accepted": true });
        let receipt: RpcReceipt = serde_json::from_value(raw).unwrap();
        assert!(receipt.accepted());
    }

    #[test]
    fn rpc_receipt_not_pending() {
        let raw = serde_json::json!({ "accepted": false, "reason": "not-pending" });
        let receipt: RpcReceipt = serde_json::from_value(raw).unwrap();
        assert!(!receipt.accepted());
    }

    #[test]
    fn server_response_with_extra_fields_tolerated() {
        // Additive schema change: a new top-level field must not break parsing.
        let raw = serde_json::json!({
            "type": "server-response",
            "rpcId": "rpc-1",
            "result": { "ok": true, "value": {} },
            "traceId": "t-9",
        });
        let resp: ServerResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.result.is_ok());
    }
}
