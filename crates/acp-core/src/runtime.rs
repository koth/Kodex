use crate::events::{ClientEvent, SessionConfig};
use crate::mapping::append_runtime_event_log;
use anyhow::Context;
use serde_json::json;
use std::sync::mpsc;
use workspace_model::UserPromptContent;

mod agent_process;
mod client_handlers;
mod codebuddy;
mod permissions;
mod process;
mod prompt_content;
mod prompt_loop;
mod session_lifecycle;
mod session_titles;
mod shutdown;
mod terminal;
#[cfg(test)]
mod tests;
mod tool_stop;
mod workspace_paths;
use agent_process::{AgentTransport, HiddenAgentProcess, RemoteSshAgentProcess, TcpAgentProcess};
pub use permissions::PermissionBroker;
pub use shutdown::ShutdownSignal;

pub enum RuntimeCommand {
    SendPrompt {
        prompt: Vec<UserPromptContent>,
        accepted_tx: Option<mpsc::Sender<anyhow::Result<()>>>,
    },
    SetConfigOption {
        config_id: String,
        value_id: String,
        provider: Option<String>,
        reply_tx: Option<mpsc::Sender<anyhow::Result<Vec<ClientEvent>>>>,
    },
    SetMode {
        mode_id: String,
        reply_tx: mpsc::Sender<anyhow::Result<Vec<ClientEvent>>>,
    },
    SetModel {
        model_id: String,
        provider: Option<String>,
        reply_tx: mpsc::Sender<anyhow::Result<Vec<ClientEvent>>>,
    },
    ResolveCodeBuddyInterruption {
        session_id: String,
        tool_call_id: String,
        decision: String,
        reply_tx: mpsc::Sender<anyhow::Result<()>>,
    },
    /// Answer a DeepSeek Harness approval/question (server-request) by its `rpcId`.
    /// The bridge POSTs a `ClientResponse` to `/api/respond` over the shared HTTP client.
    ResolveHarnessApproval {
        rpc_id: String,
        result: HarnessApprovalResult,
        reply_tx: mpsc::Sender<anyhow::Result<()>>,
    },
    CancelPrompt {
        reply_tx: Option<mpsc::Sender<anyhow::Result<()>>>,
    },
    StopTool {
        tool_call_id: String,
        reply_tx: mpsc::Sender<anyhow::Result<Vec<ClientEvent>>>,
    },
    Shutdown,
}

/// User decision for a harness approval/question, carried back through
/// `RuntimeCommand::ResolveHarnessApproval` to the bridge's `/api/respond` POST.
#[derive(Debug, Clone)]
pub enum HarnessApprovalResult {
    /// Approval: `allowed-once` or `rejected`.
    Approval { approval_id: String, outcome: HarnessApprovalOutcome },
    /// Question answer batch (one answer per question id).
    Question { answers: Vec<HarnessQuestionAnswer> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessApprovalOutcome {
    AllowedOnce,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct HarnessQuestionAnswer {
    pub question_id: String,
    pub selected: Vec<String>,
    pub custom: Option<String>,
}

/// Backend contract for a non-ACP session transport (DeepSeek Harness host RPC).
/// Implemented by `dsh-bridge` and registered at process init via
/// [`set_harness_backend`], so `acp-core` can dispatch without a compile-time
/// dependency on the bridge crate (inversion of control breaks the cycle).
pub trait HarnessBackend: Send + Sync + 'static {
    fn run_session(
        &self,
        config: SessionConfig,
        tx_events: mpsc::Sender<ClientEvent>,
        rx_commands: mpsc::Receiver<RuntimeCommand>,
        permission_broker: PermissionBroker,
        shutdown_signal: ShutdownSignal,
    ) -> anyhow::Result<()>;
}

static HARNESS_BACKEND: std::sync::OnceLock< std::sync::Arc<dyn HarnessBackend>> =
    std::sync::OnceLock::new();

/// Register the harness backend implementation. Called once at process startup
/// by the crate that links `dsh-bridge` (e.g. `app-core` or the desktop shell).
/// Subsequent calls are ignored; the first registration wins.
pub fn set_harness_backend(backend: std::sync::Arc<dyn HarnessBackend>) {
    let _ = HARNESS_BACKEND.set(backend);
}

fn harness_backend() -> Option<std::sync::Arc<dyn HarnessBackend>> {
    HARNESS_BACKEND.get().cloned()
}

pub(crate) fn run_session(
    config: SessionConfig,
    tx_events: mpsc::Sender<ClientEvent>,
    rx_commands: mpsc::Receiver<RuntimeCommand>,
    permission_broker: PermissionBroker,
    shutdown_signal: ShutdownSignal,
) -> anyhow::Result<()> {
    let log_config = config.clone();

    // The DeepSeek Harness backend drives its own multi-thread tokio runtime
    // and uses `host.runtime().block_on(...)` for control calls. Running it
    // inside an outer `block_on` would nest runtimes and panic with
    // "Cannot start a runtime from within a runtime", wedging the worker
    // thread so `session_config` never hydrates. Dispatch it directly on the
    // worker thread instead of entering an outer runtime.
    if config.harness_endpoint.is_some()
        && let Some(backend) = harness_backend()
    {
        let result = backend.run_session(
            config,
            tx_events,
            rx_commands,
            permission_broker,
            shutdown_signal,
        );
        let payload = match &result {
            Ok(()) => json!({ "status": "ok" }),
            Err(error) => json!({ "status": "error", "error": error.to_string() }),
        };
        let _ = append_runtime_event_log(&log_config, "runtime/session_result", &payload);
        return result;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = append_runtime_event_log(
                &log_config,
                "runtime/session_result",
                &json!({
                    "status": "error",
                    "error": format!("failed to create tokio runtime: {err}")
                }),
            );
            return Err(err).context("failed to create tokio runtime");
        }
    };

    let result: anyhow::Result<()> = runtime.block_on(async move {
        let agent = if config.remote_ssh.is_some() {
            AgentTransport::RemoteSsh(
                RemoteSshAgentProcess::from_config(&config)?
                    .shutdown_signal(shutdown_signal.clone()),
            )
        } else if config.acp_port > 0 {
            AgentTransport::Tcp(
                TcpAgentProcess::from_config(&config)?.shutdown_signal(shutdown_signal.clone()),
            )
        } else {
            AgentTransport::Stdio(
                HiddenAgentProcess::from_config(&config)?.shutdown_signal(shutdown_signal.clone()),
            )
        };
        client_handlers::connect_agent_client(
            agent,
            config,
            tx_events,
            rx_commands,
            permission_broker,
            shutdown_signal,
        )
        .await?;

        Ok(())
    });

    let payload = match &result {
        Ok(()) => json!({ "status": "ok" }),
        Err(error) => json!({ "status": "error", "error": error.to_string() }),
    };
    let _ = append_runtime_event_log(&log_config, "runtime/session_result", &payload);

    result
}
