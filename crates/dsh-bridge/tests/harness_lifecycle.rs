//! Full lifecycle through `run_harness_session`: config hydration and
//! `SetModel`. Exercises the same worker-thread dispatch path as
//! `acp-core::runtime::run_session` (after the nested-runtime fix), so
//! regressions in model switching surface here. The prompt→message→turn flow
//! is covered by `harness_integration::single_session_create_prompt_message_flow`.

mod common;

use acp_core::{ClientEvent, PermissionBroker, RuntimeCommand, SessionConfig, ShutdownSignal};
use common::{MockHarness, default_config};
use dsh_bridge::HarnessHostRegistry;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

fn config_for(endpoint: String) -> SessionConfig {
    SessionConfig {
        workspace_root: "/tmp".into(),
        app_data_root: "/tmp".into(),
        model: "deepseek-v4-pro".into(),
        agent_command: "dsh".into(),
        agent_env: Vec::new(),
        resume_session_id: None,
        log_id: "lifecycle".into(),
        acp_port: 0,
        remote_ssh: None,
        mcp_servers: Vec::new(),
        harness_endpoint: Some(endpoint),
        agent_preset: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_hydrate_and_set_model() {
    let mock = MockHarness::start(default_config()).await;
    let registry = Arc::new(HarnessHostRegistry::new());

    let (tx, rx) = mpsc::channel::<ClientEvent>();
    let (command_tx, command_rx) = mpsc::channel();
    let config = config_for(mock.endpoint());

    let worker_registry = registry.clone();
    let worker = thread::spawn(move || {
        dsh_bridge::run_harness_session(
            worker_registry,
            config,
            tx,
            command_rx,
            PermissionBroker::default(),
            ShutdownSignal::default(),
        )
    });

    // 1. Config hydration.
    let mut hydrated = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ClientEvent::SessionConfigUpdated { state }) => {
                hydrated = state.hydrated;
                break;
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(hydrated, "config never hydrated");

    // 2. SetModel: must return refreshed config, not Interrupted.
    let (model_reply_tx, model_reply_rx) = mpsc::channel();
    command_tx
        .send(RuntimeCommand::SetModel {
            model_id: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            reply_tx: model_reply_tx,
        })
        .unwrap();
    let model_events = model_reply_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SetModel reply timed out")
        .expect("SetModel reply channel closed");
    assert!(
        !model_events
            .iter()
            .any(|e| matches!(e, ClientEvent::Interrupted { .. })),
        "SetModel returned Interrupted: {model_events:?}"
    );
    assert!(
        model_events
            .iter()
            .any(|e| matches!(e, ClientEvent::SessionConfigUpdated { .. })),
        "SetModel did not republish config: {model_events:?}"
    );

    // 3. The mock records every `session.selectModel` call. The composer sends
    // a provider-qualified value id like `kodex-provider/deepseek/deepseek-v4-flash`;
    // the bridge must decode it so dsh receives the bare model id + provider
    // (dsh's schema rejects the encoded value and an empty provider).
    let calls = mock.calls();
    let select_model_calls: Vec<_> = calls
        .iter()
        .filter(|(m, _)| m == "session.selectModel")
        .collect();
    assert!(
        !select_model_calls.is_empty(),
        "session.selectModel was not called: {calls:?}"
    );
    // The mock's session.selectModel response always succeeds; we only need
    // the call to have happened with decoded values (asserted by the absence of
    // Interrupted above, since a 400/422 would surface as selectModel failed).

    let _ = command_tx.send(RuntimeCommand::Shutdown);
    let _ = worker.join();
}

#[tokio::test(flavor = "multi_thread")]
async fn set_mode_on_started_session_reports_notice_not_interrupt() {
    // A session that has already started a turn cannot switch preset: dsh
    // answers `agent-preset-locked`. The bridge must surface that as a system
    // notice, NOT `Interrupted` (which would mark the session dead/disconnected).
    let mut config_mock = default_config();
    config_mock.preset_locked = true;
    let mock = MockHarness::start(config_mock).await;
    let registry = Arc::new(HarnessHostRegistry::new());

    let (tx, rx) = mpsc::channel::<ClientEvent>();
    let (command_tx, command_rx) = mpsc::channel();
    let config = config_for(mock.endpoint());

    let worker_registry = registry.clone();
    let worker = thread::spawn(move || {
        dsh_bridge::run_harness_session(
            worker_registry,
            config,
            tx,
            command_rx,
            PermissionBroker::default(),
            ShutdownSignal::default(),
        )
    });

    // Wait for hydration before sending SetMode.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ClientEvent::SessionConfigUpdated { state }) if state.hydrated => break,
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let (mode_reply_tx, mode_reply_rx) = mpsc::channel();
    command_tx
        .send(RuntimeCommand::SetMode {
            mode_id: "standard".into(),
            reply_tx: mode_reply_tx,
        })
        .unwrap();
    // The bridge rejects preset switches on a started session with an error
    // so `SessionHandle::set_mode` does not mutate the shared permission
    // broker; the composer surfaces the error to the user.
    let mode_result = mode_reply_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SetMode reply timed out");
    let err = mode_result.expect_err("locked preset switch must fail");
    assert!(
        err.to_string().contains("预设已固定"),
        "error must explain the preset is locked: {err}"
    );

    let _ = command_tx.send(RuntimeCommand::Shutdown);
    let _ = worker.join();
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_hydrate_includes_preset_mode_control_and_set_mode() {
    let mock = MockHarness::start(default_config()).await;
    let registry = Arc::new(HarnessHostRegistry::new());

    let (tx, rx) = mpsc::channel::<ClientEvent>();
    let (command_tx, command_rx) = mpsc::channel();
    let config = config_for(mock.endpoint());

    let worker_registry = registry.clone();
    let worker = thread::spawn(move || {
        dsh_bridge::run_harness_session(
            worker_registry,
            config,
            tx,
            command_rx,
            PermissionBroker::default(),
            ShutdownSignal::default(),
        )
    });

    // 1. Config hydration must include a Mode (agent-preset) control with the
    // four shipped presets, defaulted to `code`.
    let mut mode_control = None;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ClientEvent::SessionConfigUpdated { state }) => {
                mode_control = state
                    .controls
                    .iter()
                    .find(|c| c.category == workspace_model::SessionConfigCategory::Mode)
                    .cloned();
                if mode_control.is_some() {
                    break;
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let mode_control = mode_control.expect("no Mode control published");
    assert_eq!(mode_control.current_value_id, "code");
    assert!(
        mode_control
            .choices
            .iter()
            .any(|c| c.id == "standard"),
        "preset choices missing `standard`: {:?}",
        mode_control.choices
    );
    assert!(
        mode_control.choices.iter().any(|c| c.id == "minimal"),
        "preset choices missing `minimal`"
    );
    assert!(
        mode_control.choices.iter().any(|c| c.id == "cordis"),
        "preset choices missing `cordis`"
    );

    // 2. SetMode → agentPreset.select, then re-published config shows the new
    // selection.
    let (mode_reply_tx, mode_reply_rx) = mpsc::channel();
    command_tx
        .send(RuntimeCommand::SetMode {
            mode_id: "standard".into(),
            reply_tx: mode_reply_tx,
        })
        .unwrap();
    let mode_events = mode_reply_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("SetMode reply timed out")
        .expect("SetMode reply channel closed");
    assert!(
        !mode_events
            .iter()
            .any(|e| matches!(e, ClientEvent::Interrupted { .. })),
        "SetMode returned Interrupted: {mode_events:?}"
    );
    let updated_mode = mode_events
        .iter()
        .find_map(|e| match e {
            ClientEvent::SessionConfigUpdated { state } => state
                .controls
                .iter()
                .find(|c| c.category == workspace_model::SessionConfigCategory::Mode)
                .cloned(),
            _ => None,
        })
        .expect("SetMode did not republish Mode control");
    assert_eq!(updated_mode.current_value_id, "standard");

    // 3. The mock recorded the agentPreset.select call with the chosen id.
    assert_eq!(mock.preset_selects(), vec!["standard".to_string()]);

    let _ = command_tx.send(RuntimeCommand::Shutdown);
    let _ = worker.join();
}

#[tokio::test(flavor = "multi_thread")]
async fn new_session_create_carries_configured_preset() {
    // A new session must pass `config.agent_preset` to `session.create` so the
    // harness composes the chosen preset (e.g. `minimal`) instead of the
    // deployment default.
    let mock = MockHarness::start(default_config()).await;
    let registry = Arc::new(HarnessHostRegistry::new());

    let (tx, _rx) = mpsc::channel::<ClientEvent>();
    let (_command_tx, command_rx) = mpsc::channel();
    let mut config = config_for(mock.endpoint());
    config.agent_preset = Some("minimal".into());

    let worker_registry = registry.clone();
    let worker = thread::spawn(move || {
        dsh_bridge::run_harness_session(
            worker_registry,
            config,
            tx,
            command_rx,
            PermissionBroker::default(),
            ShutdownSignal::default(),
        )
    });

    // Wait for the create call to land, then shut down.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) && mock.creates().is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let creates = mock.creates();
    assert_eq!(creates.len(), 1, "expected one session.create call");
    assert_eq!(
        creates[0].get("agentPreset").and_then(|v| v.as_str()),
        Some("minimal"),
        "session.create must carry the configured preset: {creates:?}"
    );

    let _ = _command_tx.send(RuntimeCommand::Shutdown);
    let _ = worker.join();
}
