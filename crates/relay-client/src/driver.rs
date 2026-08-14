//! The relay driver: ties the connection (frame pipe + E2E) to a control
//! handler and an event source, running the inbound (request -> response)
//! and outbound (event push) loops concurrently.
//!
//! Layering: this is transport-only. `ControlHandler` and `EventSource`
//! are traits declared here over `relay-protocol` types, so the driver can
//! be unit-tested with mocks. The shell adapts `DesktopRemoteControl`
//! (impl `app_core::RemoteControl`) to `ControlHandler`, and bridges
//! `Application::subscribe_updates` + `UiPatchCursor` into `EventSource`.

use anyhow::Result;
use relay_protocol::{ControlRequest, ControlResponse, Envelope, Message, PairingConfirm};
use std::sync::{Arc, Mutex};

use crate::connection::RelayConnection;
use crate::crypto::SessionKey;
use crate::RelayTransport;

/// Handles an inbound `ControlRequest`, returning the matching
/// `ControlResponse` (or an `Error` response on failure).
pub trait ControlHandler: Send {
    fn handle(
        &mut self,
        request: ControlRequest,
    ) -> impl std::future::Future<Output = ControlResponse> + Send;
}

/// Produces outbound event envelopes (already-wrapped `EventFrame`
/// messages). Returns `None` when the event stream is exhausted; the
/// driver then continues the inbound loop alone.
pub trait EventSource: Send {
    fn next_event(
        &mut self,
    ) -> impl std::future::Future<Output = Option<Envelope>> + Send;
}

/// Derives the E2E session key from a `PairingConfirm` received over the
/// relay. The shell implements this with the PC's static X25519 secret and
/// the phone's ephemeral public key carried in `session_key_material`.
pub trait PairingHandler: Send {
    fn derive_session_key(
        &mut self,
        confirm: PairingConfirm,
    ) -> impl std::future::Future<Output = Result<(SessionKey, String)>> + Send;
}

/// Drives a relay connection: routes inbound control requests to a
/// `ControlHandler` and pushes outbound events from an `EventSource`,
/// both over the same E2E connection. Fail-open: any connection error
/// ends `run` without panicking; local sessions are unaffected.
pub struct RelayDriver<T: RelayTransport, H: ControlHandler, E: EventSource, P: PairingHandler> {
    conn: RelayConnection<T>,
    handler: H,
    events: E,
    pairing: P,
    session_sink: Arc<Mutex<Option<(SessionKey, String)>>>,
}

impl<T: RelayTransport, H: ControlHandler, E: EventSource, P: PairingHandler> RelayDriver<T, H, E, P> {
    pub fn new(conn: RelayConnection<T>, handler: H, events: E, pairing: P) -> Self {
        Self::new_with_session_sink(
            conn,
            handler,
            events,
            pairing,
            Arc::new(Mutex::new(None)),
        )
    }

    /// Same as [`RelayDriver::new`], but records the most recently installed
    /// E2E session key so a caller-owned reconnect loop can reinstall it on
    /// the next connection without a fresh pairing handshake.
    pub fn new_with_session_sink(
        conn: RelayConnection<T>,
        handler: H,
        events: E,
        pairing: P,
        session_sink: Arc<Mutex<Option<(SessionKey, String)>>>,
    ) -> Self {
        Self {
            conn,
            handler,
            events,
            pairing,
            session_sink,
        }
    }

    /// Run the inbound + outbound loops until the connection closes or
    /// errors. Returns Ok on clean close, Err on a connection failure
    /// (caller may reconnect).
    pub async fn run(mut self) -> Result<()> {
        let mut outbound_done = false;
        let heartbeat = Envelope::from_message(None, &Message::Heartbeat).ok();
        let heartbeat_interval = std::time::Duration::from_millis(
            (self.conn.heartbeat().as_millis() / 2).max(1000) as u64,
        );
        let mut heartbeat_tick = tokio::time::interval(heartbeat_interval);
        loop {
            if outbound_done {
                match self.conn.recv_envelope().await? {
                    None => return Ok(()),
                    Some(envelope) => self.handle_inbound(envelope).await?,
                }
            } else {
                tokio::select! {
                    inbound = self.conn.recv_envelope() => {
                        match inbound? {
                            None => return Ok(()),
                            Some(envelope) => self.handle_inbound(envelope).await?,
                        }
                    }
                    outbound = self.events.next_event() => {
                        match outbound {
                            None => outbound_done = true,
                            Some(envelope) => self.conn.send_envelope(&envelope).await?,
                        }
                    }
                    _ = heartbeat_tick.tick() => {
                        if let Some(heartbeat) = &heartbeat {
                            self.conn.send_envelope(heartbeat).await?;
                        }
                    }
                }
            }
        }
    }

    async fn handle_inbound(&mut self, envelope: Envelope) -> Result<()> {
        let request_id = envelope.id;
        let message = match envelope.into_message() {
            Ok(message) => message,
            Err(_) => return Ok(()),
        };
        match message {
            Message::ControlRequest(request) => {
                let has_key = self.conn.has_session_key();
                eprintln!("[remote-control] received ControlRequest op={:?} has_key={}", std::mem::discriminant(&request), has_key);
                let response = self.handler.handle(request).await;
                let reply = Envelope::from_message(request_id, &Message::ControlResponse(response))?;
                eprintln!("[remote-control] sending ControlResponse");
                self.conn.send_envelope(&reply).await
            }
            Message::PairingConfirm(confirm) => {
                eprintln!("[remote-control] received PairingConfirm: phone={}, pc={}, material_len={}", confirm.phone_device_id, confirm.pc_device_id, confirm.session_key_material.len());
                // The relay forwards the phone's ephemeral public key in
                // `session_key_material`. Derive the E2E session key and
                // install it so subsequent control requests decrypt.
                let (key, peer_device_id) = match self.pairing.derive_session_key(confirm).await {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[remote-control] pairing key derivation failed: {e}");
                        return Err(e);
                    }
                };
                eprintln!("[remote-control] installed session key, peer={}", peer_device_id);
                self.conn.install_session_key(key, peer_device_id);
                if let Ok(mut guard) = self.session_sink.lock() {
                    *guard = self
                        .conn
                        .session_key()
                        .zip(self.conn.peer_device_id());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_protocol::{ControlRequest, ControlResponse, EventFrame, Message};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use uuid::Uuid;
    use workspace_model::SessionStatus;

    /// In-memory transport: a pair of mpsc channels cross-linked so A's
    /// `send_text` lands in B's `recv_text` and vice versa. Avoids real
    /// WebSocket / split-sink deadlocks; tests driver routing logic
    /// (E2E + WS are validated separately in `connection` tests).
    struct ChannelTransport {
        tx: mpsc::Sender<String>,
        rx: mpsc::Receiver<String>,
    }

    impl RelayTransport for ChannelTransport {
        async fn send_text(&mut self, frame: String) -> Result<()> {
            self.tx
                .send(frame)
                .await
                .map_err(|e| anyhow::anyhow!("channel send: {e}"))
        }
        async fn recv_text(&mut self) -> Result<Option<String>> {
            Ok(self.rx.recv().await)
        }
        async fn close(&mut self) {}
    }

    /// Cross-link two in-memory connections: pc.send -> phone.recv and
    /// phone.send -> pc.recv.
    fn linked_pair() -> (
        RelayConnection<ChannelTransport>,
        RelayConnection<ChannelTransport>,
    ) {
        let (pc_tx, phone_rx) = mpsc::channel(32);
        let (phone_tx, pc_rx) = mpsc::channel(32);
        let pc = RelayConnection::new(
            ChannelTransport {
                tx: pc_tx,
                rx: pc_rx,
            },
            Duration::from_secs(30),
        );
        let phone = RelayConnection::new(
            ChannelTransport {
                tx: phone_tx,
                rx: phone_rx,
            },
            Duration::from_secs(30),
        );
        (pc, phone)
    }

    /// Handler that records requests and replies with the matching response
    /// variant (Cancel/StopTool) or an Error for unsupported ops.
    struct EchoHandler {
        seen: Vec<ControlRequest>,
    }

    impl ControlHandler for EchoHandler {
        async fn handle(&mut self, request: ControlRequest) -> ControlResponse {
            let request_id = request.request_id();
            self.seen.push(request.clone());
            match request {
                ControlRequest::Cancel { .. } => ControlResponse::Cancel { request_id },
                ControlRequest::StopTool { .. } => ControlResponse::StopTool { request_id },
                _ => ControlResponse::Error {
                    request_id,
                    message: "unsupported in mock".to_string(),
                },
            }
        }
    }

    /// Pairing handler that never derives a key (no pairing in these tests).
    struct NoopPairing;

    impl PairingHandler for NoopPairing {
        async fn derive_session_key(
            &mut self,
            _confirm: PairingConfirm,
        ) -> Result<(SessionKey, String)> {
            anyhow::bail!("no pairing expected in this test")
        }
    }

    /// Event source that yields N canned event envelopes then stops.
    struct FixedEvents {
        frames: Vec<Envelope>,
    }

    impl EventSource for FixedEvents {
        async fn next_event(&mut self) -> Option<Envelope> {
            if self.frames.is_empty() {
                None
            } else {
                Some(self.frames.remove(0))
            }
        }
    }

    fn event_envelope(frame: EventFrame) -> Envelope {
        Envelope::from_message(None, &Message::Event(frame)).unwrap()
    }

    #[tokio::test]
    async fn driver_routes_cancel_request_to_handler_and_responds() {
        let (pc_conn, mut phone) = linked_pair();
        let handler = EchoHandler {
            seen: Vec::new(),
        };
        let events = FixedEvents {
            frames: Vec::new(),
        };
        let driver = RelayDriver::new(pc_conn, handler, events, NoopPairing);
        let task = tokio::spawn(async move { driver.run().await });

        let request_id = Uuid::new_v4();
        let request = ControlRequest::Cancel { request_id };
        let req_env =
            Envelope::from_message(Some(request_id), &Message::ControlRequest(request)).unwrap();
        phone.send_envelope(&req_env).await.unwrap();

        let env = phone
            .recv_envelope()
            .await
            .unwrap()
            .expect("driver should respond");
        match env.into_message().unwrap() {
            Message::ControlResponse(ControlResponse::Cancel { request_id: rid }) => {
                assert_eq!(rid, request_id);
            }
            other => panic!("expected Cancel response, got {other:?}"),
        }
        task.abort();
    }

    #[tokio::test]
    async fn driver_routes_stop_tool_request() {
        let (pc_conn, mut phone) = linked_pair();
        let handler = EchoHandler {
            seen: Vec::new(),
        };
        let events = FixedEvents {
            frames: Vec::new(),
        };
        let driver = RelayDriver::new(pc_conn, handler, events, NoopPairing);
        let task = tokio::spawn(async move { driver.run().await });

        let request_id = Uuid::new_v4();
        let request = ControlRequest::StopTool {
            request_id,
            tool_call_id: "tool-7".to_string(),
        };
        let req_env =
            Envelope::from_message(Some(request_id), &Message::ControlRequest(request)).unwrap();
        phone.send_envelope(&req_env).await.unwrap();

        let env = phone
            .recv_envelope()
            .await
            .unwrap()
            .expect("driver should respond");
        match env.into_message().unwrap() {
            Message::ControlResponse(ControlResponse::StopTool { request_id: rid }) => {
                assert_eq!(rid, request_id);
            }
            other => panic!("expected StopTool response, got {other:?}"),
        }
        task.abort();
    }

    #[tokio::test]
    async fn driver_pushes_events_to_phone() {
        let (pc_conn, mut phone) = linked_pair();
        let handler = EchoHandler {
            seen: Vec::new(),
        };
        let event = event_envelope(EventFrame::SessionStatusChanged {
            session_id: "s-1".to_string(),
            status: SessionStatus::Idle,
        });
        let events = FixedEvents {
            frames: vec![event],
        };
        let driver = RelayDriver::new(pc_conn, handler, events, NoopPairing);
        let task = tokio::spawn(async move { driver.run().await });

        let env = phone
            .recv_envelope()
            .await
            .unwrap()
            .expect("driver should push the event");
        match env.into_message().unwrap() {
            Message::Event(EventFrame::SessionStatusChanged { session_id, .. }) => {
                assert_eq!(session_id, "s-1");
            }
            other => panic!("expected SessionStatusChanged event, got {other:?}"),
        }
        task.abort();
    }

    #[tokio::test]
    async fn driver_ends_cleanly_when_connection_closes() {
        // Fail-open / relay-down: dropping the phone side closes the PC's
        // recv channel; the driver's recv returns None and run() returns
        // Ok without panicking. Local state (handler) is untouched.
        let (pc_conn, phone) = linked_pair();
        drop(phone);
        let handler = EchoHandler {
            seen: Vec::new(),
        };
        let events = FixedEvents {
            frames: Vec::new(),
        };
        let driver = RelayDriver::new(pc_conn, handler, events, NoopPairing);
        let result = tokio::time::timeout(Duration::from_secs(5), driver.run()).await;
        assert!(result.is_ok(), "driver run completes (does not hang)");
    }

    #[tokio::test]
    async fn driver_routes_request_over_e2e_encrypted_link() {
        // Same routing test but with a SessionKey installed on both sides:
        // the channel carries EncryptedEnvelope ciphertext, proving the
        // driver + connection E2E path end-to-end.
        let (mut pc_conn, mut phone) = linked_pair();
        let key = crate::SessionKey::derive(b"pairing-secret", b"kodex-relay-salt");
        pc_conn.install_session_key(key.clone(), "phone".to_string());
        phone.install_session_key(key, "pc".to_string());

        let handler = EchoHandler {
            seen: Vec::new(),
        };
        let events = FixedEvents {
            frames: Vec::new(),
        };
        let driver = RelayDriver::new(pc_conn, handler, events, NoopPairing);
        let task = tokio::spawn(async move { driver.run().await });

        let request_id = Uuid::new_v4();
        let request = ControlRequest::Cancel { request_id };
        let req_env =
            Envelope::from_message(Some(request_id), &Message::ControlRequest(request)).unwrap();
        phone.send_envelope(&req_env).await.unwrap();

        let env = phone
            .recv_envelope()
            .await
            .unwrap()
            .expect("driver should respond over E2E");
        match env.into_message().unwrap() {
            Message::ControlResponse(ControlResponse::Cancel { request_id: rid }) => {
                assert_eq!(rid, request_id);
            }
            other => panic!("expected Cancel response over E2E, got {other:?}"),
        }
        task.abort();
    }
}
