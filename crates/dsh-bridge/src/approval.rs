//! Approval/question bridging: translate harness `approval/requested` /
//! `question/requested` into Kodex `ToolPermissionRequest`, and carry the
//! user's decision back to `/api/respond` via `RuntimeCommand::ResolveHarnessApproval`.
//!
//! Pending entries are keyed by the dsh `rpcId`/`approvalId` (globally unique
//! UUID), stored in the session's own `PermissionBroker`-adjacent table on the
//! `SessionSink`. The harness's global pending registry cross-checks
//! `sessionId` on respond, so a misrouted answer is rejected as `bad-response`.

use acp_core::{HarnessApprovalOutcome, HarnessApprovalResult, HarnessQuestionAnswer};

use crate::host::{PendingApprovalKind, SessionSink};
use crate::rpc_types::{
    ApprovalOutcomeWire, ApprovalResponsePayload, AskUserQuestionAnswerItemWire,
    AskUserQuestionAnswerWire, ClientResponse, QuestionResponsePayload, RpcId,
};

/// Snapshot of a session's pending approvals/questions, used by the session
/// loop to build the `/api/respond` payload for a `ResolveHarnessApproval`.
#[derive(Debug, Default, Clone)]
pub struct PendingApprovals {
    entries: Vec<PendingEntryView>,
    question_rpc_ids: Vec<(String, RpcId)>,
}

#[derive(Debug, Clone)]
struct PendingEntryView {
    pub ui_id: String,
    pub kind: PendingApprovalKind,
    pub approval_id: String,
}

impl PendingApprovals {
    pub fn from_entries(
        entries: Vec<crate::host::PendingEntry>,
        question_rpc_ids: Vec<(String, RpcId)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|e| PendingEntryView {
                    ui_id: e.ui_id,
                    kind: e.kind,
                    approval_id: e.approval_id,
                })
                .collect(),
            question_rpc_ids,
        }
    }

    /// Build the `ClientResponse` for a resolved approval/question, looking up
    /// the dsh `rpcId` and `sessionId` from the session sink.
    pub fn build_response(
        &self,
        sink: &SessionSink,
        rpc_id: &str,
        result: &HarnessApprovalResult,
    ) -> Option<ClientResponse> {
        let session_id = sink.session_id()?;
        // Find the pending entry by the ui_id (== approvalId for approvals; ==
        // first question id for questions).
        let entry = self.entries.iter().find(|e| e.ui_id == rpc_id)?;
        match (entry.kind, result) {
            (
                PendingApprovalKind::Approval,
                HarnessApprovalResult::Approval {
                    approval_id,
                    outcome,
                },
            ) => {
                let wire_outcome = match outcome {
                    HarnessApprovalOutcome::AllowedOnce => ApprovalOutcomeWire::AllowedOnce,
                    HarnessApprovalOutcome::Rejected => ApprovalOutcomeWire::Rejected,
                };
                let payload = ApprovalResponsePayload {
                    session_id,
                    approval_id: approval_id.clone(),
                    outcome: wire_outcome,
                };
                let value = serde_json::to_value(&payload).ok()?;
                Some(ClientResponse::ok(rpc_id.to_string(), value))
            }
            (PendingApprovalKind::Question, HarnessApprovalResult::Question { answers }) => {
                // The respond rpcId is the question/requested ServerRequest's
                // rpcId, not the UI-facing question id: the harness matches
                // the pending ask by rpcId and rejects any other id as
                // `bad-response` (which surfaced as a silent hang — the
                // question stayed open forever).
                let respond_rpc_id = self
                    .question_rpc_ids
                    .iter()
                    .find(|(id, _)| id == rpc_id)
                    .map(|(_, rpc_id)| rpc_id.clone())?;
                let wire_answers: Vec<AskUserQuestionAnswerItemWire> = answers
                    .iter()
                    .map(|a: &HarnessQuestionAnswer| AskUserQuestionAnswerItemWire {
                        id: a.question_id.clone(),
                        selected: a.selected.clone(),
                        custom: a.custom.clone(),
                    })
                    .collect();
                let payload = QuestionResponsePayload {
                    session_id,
                    answer: AskUserQuestionAnswerWire {
                        answers: wire_answers,
                    },
                };
                let value = serde_json::to_value(&payload).ok()?;
                Some(ClientResponse::ok(respond_rpc_id, value))
            }
            // Kind/result mismatch — the UI sent the wrong shape for this id.
            _ => None,
        }
    }
}
