//! Bridge integration tests against a fake dsh web host.
//!
//! Covers the shared-host concurrency model without a real `dsh web` binary:
//! frame routing to per-session sinks, unmatched-frame drop, concurrent
//! control POSTs, approval round-trips over `/api/respond`, SSE reconnection
//! with multi-session re-baseline, and host lifetime across sessions.

mod common;

use acp_core::{ClientEvent, PermissionBroker};
use common::{
    MockHarness, MuxEnd, MuxScript, default_config, history_event, mux_session_event,
    mux_subscribed, scripts_with,
};
use dsh_bridge::{HarnessHostRegistry, HttpClient};
use serde_json::json;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

fn test_sink() -> (Arc<dsh_bridge::SessionSink>, mpsc::Receiver<ClientEvent>) {
    let (tx, rx) = mpsc::channel();
    (Arc::new(dsh_bridge::SessionSink::new(tx, PermissionBroker::default())), rx)
}

fn drain_events(rx: &mpsc::Receiver<ClientEvent>, deadline: Duration) -> Vec<ClientEvent> {
    let mut events = Vec::new();
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => events.push(event),
            Err(mpsc::RecvTimeoutError::Timeout) if events.is_empty() => continue,
            Err(_) => break,
        }
    }
    events
}

fn wait_until<F: Fn() -> bool>(f: F, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_share_one_host_and_route_frames_to_the_right_sink() {
    // Mock host streams frames for session A and B on the single mux stream.
    let config = {
        let mut c = default_config();
        c.mux = scripts_with(vec![
            mux_subscribed("s-a", 0),
            mux_subscribed("s-b", 0),
            mux_session_event(
                "s-a",
                1,
                "assistant/message",
                json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "hello A" }] } }),
            ),
            mux_session_event(
                "s-b",
                2,
                "assistant/message",
                json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "hello B" }] } }),
            ),
        ]);
        c
    };
    let mock = MockHarness::start(config).await;

    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();

    let (sink_a, rx_a) = test_sink();
    sink_a.set_session_id("s-a".into());
    host.router().register("s-a".into(), sink_a);
    let (sink_b, rx_b) = test_sink();
    sink_b.set_session_id("s-b".into());
    host.router().register("s-b".into(), sink_b);

    // Frames are delivered to the owning session only.
    let events_a = drain_events(&rx_a, Duration::from_millis(500));
    let events_b = drain_events(&rx_b, Duration::from_millis(500));
    assert!(
        events_a.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "hello A")),
        "session A did not receive its frame: {events_a:?}"
    );
    assert!(
        events_b.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "hello B")),
        "session B did not receive its frame: {events_b:?}"
    );
    assert!(
        !events_a.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "hello B")),
        "session A received session B's frame"
    );

    // One host, one mux connection, one endpoint — the registry reuses it.
    let host_again = registry.acquire(mock.endpoint()).unwrap();
    assert!(Arc::ptr_eq(&host, &host_again));
    assert_eq!(mock.mux_connection_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn unmatched_frame_is_dropped_not_fatal() {
    let mut c = default_config();
    c.mux = scripts_with(vec![
        mux_subscribed("s-a", 0),
        // Frame for a session nobody registered.
        mux_session_event(
            "s-ghost",
            1,
            "assistant/message",
            json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "ghost" }] } }),
        ),
        mux_session_event(
            "s-a",
            2,
            "assistant/message",
            json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "after ghost" }] } }),
        ),
    ]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();
    let (sink_a, rx_a) = test_sink();
    sink_a.set_session_id("s-a".into());
    host.router().register("s-a".into(), sink_a);

    let events = drain_events(&rx_a, Duration::from_millis(500));
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "after ghost")),
        "stream must continue past the unmatched frame: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_prompts_across_sessions_are_not_serialized() {
    let mock = MockHarness::start(default_config()).await;
    let client = HttpClient::new(&mock.endpoint()).unwrap();
    let registry = Arc::new(HarnessHostRegistry::new());
    let _host = registry.acquire(mock.endpoint()).unwrap();

    // Fire two prompts at once and verify both POSTs arrive (concurrency means
    // neither waits for the other; the mock records both).
    let client_a = client.clone();
    let client_b = client.clone();

    let a = tokio::spawn(async move {
        client_a
            .session_prompt(
                "rpc-a".into(),
                &dsh_bridge::SessionPromptPayload {
                    session_id: "s-a".into(),
                    mode: dsh_bridge::PromptMode::Queue,
                    content: vec![dsh_bridge::PromptContentPart::text("prompt A")],
                    client_time_zone: None,
                },
            )
            .await
    });
    let b = tokio::spawn(async move {
        client_b
            .session_prompt(
                "rpc-b".into(),
                &dsh_bridge::SessionPromptPayload {
                    session_id: "s-b".into(),
                    mode: dsh_bridge::PromptMode::Queue,
                    content: vec![dsh_bridge::PromptContentPart::text("prompt B")],
                    client_time_zone: None,
                },
            )
            .await
    });
    let (ra, rb) = tokio::join!(a, b);
    ra.unwrap().unwrap();
    rb.unwrap().unwrap();

    let calls = mock.calls();
    let prompts: Vec<&str> = calls
        .iter()
        .filter(|(method, _)| method == "session.prompt")
        .map(|(_, rpc_id)| rpc_id.as_str())
        .collect();
    assert!(prompts.contains(&"rpc-a"), "prompt A missing: {prompts:?}");
    assert!(prompts.contains(&"rpc-b"), "prompt B missing: {prompts:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_round_trip_posts_client_response_to_respond() {
    // Mux stream delivers approval/requested for session A.
    let mut c = default_config();
    c.mux = scripts_with(vec![
        mux_subscribed("s-a", 0),
        json!({
            "type": "server-request",
            "rpcId": "approval-rpc-1",
            "method": "approval/requested",
            "payload": {
                "type": "approval/requested",
                "sessionId": "s-a",
                "approvalId": "a-1",
                "toolName": "bash",
                "callId": "call-1",
                "reason": "shell"
            }
        }),
    ]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();
    let (sink_a, rx_a) = test_sink();
    sink_a.set_session_id("s-a".into());
    host.router().register("s-a".into(), sink_a.clone());

    let events = drain_events(&rx_a, Duration::from_millis(500));
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::ToolPermissionRequest { id, .. } if id == "a-1")),
        "approval request not surfaced: {events:?}"
    );

    // Bridge answers by POSTing a ClientResponse to /api/respond with the
    // approval payload, and the harness emits approval/resolved afterwards.
    let client = host.client().clone();
    let response = dsh_bridge::ClientResponse::ok(
        "approval-rpc-1".into(),
        json!({
            "sessionId": "s-a",
            "approvalId": "a-1",
            "outcome": "allowed-once"
        }),
    );
    client.respond(&response).await.unwrap();
    assert_eq!(mock.responds().len(), 1);
    assert_eq!(
        mock.responds()[0]["result"]["value"]["approvalId"],
        "a-1"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mux_drop_rebaselines_all_live_sessions_with_bounded_concurrency() {
    // Three live sessions A/B/C. The first mux connection emits their
    // subscribed frames then closes; the bridge reconnects and re-baselines
    // each session from last_seq via session.history.
    let mut c = default_config();
    c.mux = vec![
        MuxScript {
            frames: vec![
                mux_subscribed("s-a", 0),
                mux_subscribed("s-b", 0),
                mux_subscribed("s-c", 0),
            ],
            end: MuxEnd::Close,
        },
        MuxScript {
            frames: vec![
                // Re-baseline gap events delivered on the reconnected stream.
                mux_session_event(
                    "s-a",
                    5,
                    "assistant/message",
                    json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "A recovered" }] } }),
                ),
                mux_session_event(
                    "s-c",
                    7,
                    "assistant/message",
                    json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "C recovered" }] } }),
                ),
            ],
            end: MuxEnd::Hold,
        },
    ];
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();

    let (sink_a, rx_a) = test_sink();
    let (sink_b, rx_b) = test_sink();
    let (sink_c, rx_c) = test_sink();
    for (id, sink) in [
        ("s-a", &sink_a),
        ("s-b", &sink_b),
        ("s-c", &sink_c),
    ] {
        sink.set_session_id(id.into());
        host.router().register(id.into(), sink.clone());
    }

    // Wait for the drop, then the reconnect + re-baseline.
    mock.wait_for_mux_drop().await;
    let events_a = drain_events(&rx_a, Duration::from_millis(800));
    let events_b = drain_events(&rx_b, Duration::from_millis(200));
    let events_c = drain_events(&rx_c, Duration::from_millis(800));

    assert!(
        events_a.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "A recovered")),
        "A did not recover: {events_a:?}"
    );
    assert!(
        events_c.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "C recovered")),
        "C did not recover: {events_c:?}"
    );
    // B had no gap events; it must not have been interrupted.
    assert!(
        !events_b.iter().any(|e| matches!(e, ClientEvent::Interrupted { .. })),
        "B was interrupted during re-baseline: {events_b:?}"
    );
    assert!(mock.mux_connection_count() >= 2, "mux did not reconnect");
}

#[tokio::test(flavor = "multi_thread")]
async fn per_session_history_failure_isolates_that_session() {
    // Session B's history call fails during re-baseline; A and C continue.
    let mut c = default_config();
    c.history_failures = vec!["s-b".to_string()];
    c.mux = vec![
        MuxScript {
            frames: vec![
                mux_subscribed("s-a", 0),
                mux_subscribed("s-b", 0),
                mux_subscribed("s-c", 0),
            ],
            end: MuxEnd::Close,
        },
        MuxScript {
            frames: vec![],
            end: MuxEnd::Hold,
        },
    ];
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();

    let (sink_a, rx_a) = test_sink();
    let (sink_b, rx_b) = test_sink();
    let (sink_c, rx_c) = test_sink();
    for (id, sink) in [("s-a", &sink_a), ("s-b", &sink_b), ("s-c", &sink_c)] {
        sink.set_session_id(id.into());
        host.router().register(id.into(), sink.clone());
    }

    mock.wait_for_mux_drop().await;
    let events_b = drain_events(&rx_b, Duration::from_millis(800));
    assert!(
        events_b.iter().any(|e| matches!(e, ClientEvent::Interrupted { reason } if reason.contains("history"))),
        "B should be interrupted with the history failure: {events_b:?}"
    );
    // A and C are not interrupted.
    let events_a = drain_events(&rx_a, Duration::from_millis(200));
    let events_c = drain_events(&rx_c, Duration::from_millis(200));
    assert!(!events_a.iter().any(|e| matches!(e, ClientEvent::Interrupted { .. })));
    assert!(!events_c.iter().any(|e| matches!(e, ClientEvent::Interrupted { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn last_session_exit_tears_down_host_other_session_keeps_receiving() {
    // 12.8: close session A (unregister + release) while B still has the host;
    // B keeps receiving events.
    let mut c = default_config();
    c.mux = scripts_with(vec![
        mux_subscribed("s-a", 0),
        mux_subscribed("s-b", 0),
        mux_session_event(
            "s-b",
            3,
            "assistant/message",
            json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "B alive" }] } }),
        ),
    ]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();

    let (sink_a, rx_a) = test_sink();
    let (sink_b, rx_b) = test_sink();
    sink_a.set_session_id("s-a".into());
    sink_b.set_session_id("s-b".into());
    host.router().register("s-a".into(), sink_a);
    host.router().register("s-b".into(), sink_b);

    // Session A exits: unregister and release its host refcount.
    host.router().unregister(&"s-a".to_string());
    registry.release(&mock.endpoint());

    // B still receives frames on the shared host.
    let events_b = drain_events(&rx_b, Duration::from_millis(500));
    assert!(
        events_b.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "B alive")),
        "B stopped receiving after A exited: {events_b:?}"
    );
    assert!(registry.host_alive(&mock.endpoint()), "host died while B is live");
    drop(rx_a);
}

#[tokio::test(flavor = "multi_thread")]
async fn startupprobe_fails_fast_on_unreachable_endpoint() {
    // Transport failure: endpoint is not a harness host → Interrupted, no hang.
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire("http://127.0.0.1:1".to_string()).unwrap();
    // Wait for the async probe to fail, then register: the router tells the
    // late-registered sink about the failure.
    assert!(
        wait_until(|| host.probe_failed(), Duration::from_secs(5)),
        "probe did not fail fast"
    );
    let (sink, rx) = test_sink();
    sink.set_session_id("s-x".into());
    host.router().register("s-x".into(), sink);

    let events = drain_events(&rx, Duration::from_millis(500));
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::Interrupted { .. })),
        "unreachable endpoint should surface Interrupted: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn single_session_create_prompt_message_flow() {
    // M1 bridge layer: session.create → session.prompt → assistant/message.
    let mut c = default_config();
    c.mux = scripts_with(vec![
        mux_subscribed("s-1", 0),
        mux_session_event(
            "s-1",
            1,
            "assistant/message",
            json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "text answer" }] } }),
        ),
        mux_session_event("s-1", 2, "turn/end", json!({ "turn": 1, "reason": { "kind": "completed" } })),
    ]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();
    let client = host.client().clone();
    let (sink, rx) = test_sink();
    sink.set_session_id("s-1".into());
    host.router().register("s-1".into(), sink);

    let created = client
        .session_create(
            "rpc-create".into(),
            &dsh_bridge::SessionCreatePayload {
                cwd: Some("/tmp".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(created.session_id, "s-1");

    client
        .session_prompt(
            "rpc-prompt".into(),
            &dsh_bridge::SessionPromptPayload {
                session_id: "s-1".into(),
                mode: dsh_bridge::PromptMode::Queue,
                content: vec![dsh_bridge::PromptContentPart::text("hi")],
                client_time_zone: None,
            },
        )
        .await
        .unwrap();

    let events = drain_events(&rx, Duration::from_millis(500));
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::MessageChunk { content, .. } if content == "text answer")),
        "text answer not delivered: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, ClientEvent::TurnFinished { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_call_and_result_render_as_tool_events() {
    // M2 bridge layer: tool/call + tool/result with ToolEventView → tool cards.
    let mut c = default_config();
    c.mux = scripts_with(vec![
        mux_subscribed("s-1", 0),
        json!({
            "type": "server-request",
            "rpcId": "rpc-tool-call",
            "method": "session/event",
            "payload": {
                "type": "session/event",
                "sessionId": "s-1",
                "event": {
                    "type": "tool/call",
                    "seq": 1,
                    "time": 0.0,
                    "data": { "turn": 1, "step": 1, "callId": "call-1", "name": "bash", "arguments": "{\"command\":\"ls\"}" }
                },
                "view": { "for": "call", "view": { "card": "terminal", "title": "ls" } }
            }
        }),
        json!({
            "type": "server-request",
            "rpcId": "rpc-tool-result",
            "method": "session/event",
            "payload": {
                "type": "session/event",
                "sessionId": "s-1",
                "event": {
                    "type": "tool/result",
                    "seq": 2,
                    "time": 0.0,
                    "data": {
                        "turn": 1, "step": 1,
                        "message": {
                            "role": "user",
                            "content": [{ "type": "tool-result", "toolCallId": "call-1", "content": [] }]
                        }
                    }
                },
                "view": { "for": "result", "view": { "card": "terminal", "output": "ok", "exitCode": 0 } }
            }
        }),
    ]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());
    let host = registry.acquire(mock.endpoint()).unwrap();
    let (sink, rx) = test_sink();
    sink.set_session_id("s-1".into());
    host.router().register("s-1".into(), sink);

    let events = drain_events(&rx, Duration::from_millis(500));
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::ToolStarted { id, kind, .. } if id == "call-1" && kind == "execute")),
        "tool start not delivered: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, ClientEvent::ToolCompleted { id, terminal_output, .. } if id == "call-1" && terminal_output.is_some())),
        "tool completion not delivered: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn session_restore_replays_history_before_started() {
    // M4: a resumed session replays `session.history` through the mapping
    // layer before SessionStarted, reconstructing the snapshot.
    let mut c = default_config();
    c.history_events = vec![
        history_event(
            1,
            "assistant/message",
            json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [{ "type": "text", "text": "prior answer" }] } }),
        ),
        history_event(
            2,
            "tool/call",
            json!({ "turn": 1, "step": 1, "callId": "call-prior", "name": "bash", "arguments": "{}" }),
        ),
        history_event(3, "turn/end", json!({ "turn": 1, "reason": { "kind": "completed" } })),
    ];
    c.mux = scripts_with(vec![mux_subscribed("s-1", 0)]);
    let mock = MockHarness::start(c).await;
    let registry = Arc::new(HarnessHostRegistry::new());

    let (tx, rx) = mpsc::channel::<ClientEvent>();
    let (command_tx, command_rx) = mpsc::channel();
    let config = acp_core::SessionConfig {
        workspace_root: "/tmp".into(),
        app_data_root: "/tmp".into(),
        model: "deepseek-v4-pro".into(),
        agent_command: "dsh".into(),
        agent_env: Vec::new(),
        resume_session_id: Some("s-1".into()),
        log_id: "test-log".into(),
        acp_port: 0,
        remote_ssh: None,
        mcp_servers: Vec::new(),
        harness_endpoint: Some(mock.endpoint()),
    };
    let worker_registry = registry.clone();
    let worker = std::thread::spawn(move || {
        dsh_bridge::run_harness_session(
            worker_registry,
            config,
            tx,
            command_rx,
            PermissionBroker::default(),
            acp_core::ShutdownSignal::default(),
        )
    });

    // History events replay before SessionStarted.
    let mut saw_history_text = false;
    let mut saw_started = false;
    let mut saw_prior_tool = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5)
        && !(saw_history_text && saw_prior_tool && saw_started)
    {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ClientEvent::MessageChunk { content, .. }) if content == "prior answer" => {
                saw_history_text = true;
            }
            Ok(ClientEvent::ToolStarted { id, .. }) if id == "call-prior" => {
                saw_prior_tool = true;
            }
            Ok(ClientEvent::SessionStarted { .. }) => {
                saw_started = true;
            }
            Ok(ClientEvent::TurnFinished { .. }) => {}
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(saw_history_text, "history assistant text was not replayed");
    assert!(saw_prior_tool, "history tool call was not replayed");
    assert!(saw_started, "SessionStarted was not emitted after history");

    let _ = command_tx.send(acp_core::RuntimeCommand::Shutdown);
    let _ = worker.join();
}
