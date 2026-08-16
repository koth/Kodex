//! Outbound relay connection: WS dial, device auth, E2E frame crypto,
//! heartbeat, and reconnect scaffolding.
//!
//! The transport carries raw text frames (JSON). `RelayConnection` owns an
//! optional `SessionKey`: when absent (pre-pairing) it sends/receives plain
//! `Envelope` JSON (used for the `DeviceAuth` handshake); when present
//! (post-pairing) it encrypts each `Envelope` into an `EncryptedEnvelope`
//! and vice versa, so the relay routes ciphertext only. This lets auth and
//! E2E share one transport and makes the driver end-to-end testable.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use relay_protocol::{EncryptedEnvelope, Envelope, Message};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::Connector;

use crate::crypto::{SessionKey, decrypt, encrypt};

/// Abstract duplex text-frame transport. The real client uses a TLS
/// WebSocket; tests use a plain-WS mock. Carries raw JSON text so the
/// connection layer can choose plain `Envelope` or `EncryptedEnvelope`
/// framing.
pub trait RelayTransport: Send {
    fn send_text(&mut self, frame: String) -> impl std::future::Future<Output = Result<()>> + Send;
    fn recv_text(&mut self) -> impl std::future::Future<Output = Result<Option<String>>> + Send;
    fn close(&mut self) -> impl std::future::Future<Output = ()> + Send;
}

/// A `tokio-tungstenite` WebSocket transport carrying raw text frames.
pub struct WsTransport<S> {
    stream: WebSocketStream<S>,
}

impl<S> WsTransport<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    pub fn new(stream: WebSocketStream<S>) -> Self {
        Self { stream }
    }
}

impl<S> RelayTransport for WsTransport<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    async fn send_text(&mut self, frame: String) -> Result<()> {
        self.stream
            .send(WsMessage::text(frame))
            .await
            .map_err(|e| anyhow::anyhow!("ws send: {e}"))
    }

    async fn recv_text(&mut self) -> Result<Option<String>> {
        loop {
            match self.stream.next().await {
                None => return Ok(None),
                Some(Ok(WsMessage::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(WsMessage::Ping(_))) => {
                    let _ = self.stream.send(WsMessage::Pong(vec![0u8; 0].into())).await;
                    continue;
                }
                Some(Ok(WsMessage::Close(_))) => return Ok(None),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(anyhow::anyhow!("ws recv: {e}")),
            }
        }
    }

    async fn close(&mut self) {
        let _ = self.stream.close(None).await;
    }
}

/// A relay connection with optional E2E encryption.
pub struct RelayConnection<T: RelayTransport> {
    transport: T,
    heartbeat: Duration,
    session_key: Option<SessionKey>,
    peer_device_id: Option<String>,
}

impl<T: RelayTransport> RelayConnection<T> {
    pub fn new(transport: T, heartbeat: Duration) -> Self {
        Self {
            transport,
            heartbeat,
            session_key: None,
            peer_device_id: None,
        }
    }

    /// Install the E2E session key (post-pairing). Subsequent
    /// `send_envelope`/`recv_envelope` calls encrypt/decrypt with it.
    pub fn install_session_key(&mut self, key: SessionKey, peer_device_id: String) {
        self.session_key = Some(key);
        self.peer_device_id = Some(peer_device_id);
    }

    pub fn has_session_key(&self) -> bool {
        self.session_key.is_some()
    }

    /// Mutable access to the underlying transport (tests / heartbeat
    /// plaintext framing).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Drop the installed E2E session key (e.g. after a pairing error or
    /// when a re-pair is starting). Subsequent sends revert to plaintext
    /// `Envelope` framing until a new key is installed.
    pub fn clear_session_key(&mut self) {
        self.session_key = None;
        self.peer_device_id = None;
    }

    /// Clone of the installed E2E session key, if any.
    pub fn session_key(&self) -> Option<SessionKey> {
        self.session_key.clone()
    }

    /// Clone of the peer device id bound to the E2E session key, if any.
    pub fn peer_device_id(&self) -> Option<String> {
        self.peer_device_id.clone()
    }

    /// Send an envelope: encrypt to `EncryptedEnvelope` when a session key
    /// is installed, otherwise send plain `Envelope` JSON (auth phase).
    pub async fn send_envelope(&mut self, envelope: &Envelope) -> Result<()> {
        let frame = match (&self.session_key, &self.peer_device_id) {
            (Some(key), Some(peer)) => {
                let enc = encrypt(key, peer, envelope)?;
                serde_json::to_string(&enc)?
            }
            _ => serde_json::to_string(envelope)?,
        };
        self.transport.send_text(frame).await
    }

    /// Receive the next envelope: decrypt an `EncryptedEnvelope` when a
    /// session key is installed, otherwise parse a plain `Envelope`.
    ///
    /// Mixed framing tolerance: relay-originated frames (e.g. the
    /// `SubscriptionStatus` acks, pairing errors) are always plaintext, even
    /// after a session key is installed, because the relay does not hold the
    /// E2E key. When decryption fails with a shape error (the frame is not
    /// an `EncryptedEnvelope` at all) we fall back to plaintext parsing
    /// instead of tearing down the connection.
    pub async fn recv_envelope(&mut self) -> Result<Option<Envelope>> {
        loop {
            let Some(frame) = self.transport.recv_text().await? else {
                return Ok(None);
            };
            let envelope = match &self.session_key {
                Some(key) => {
                    // Plaintext frames never carry `to_device_id` at top
                    // level; encrypted envelopes always do. Route on shape.
                    if !frame.contains("\"to_device_id\"") {
                        match serde_json::from_str(&frame) {
                            Ok(env) => env,
                            Err(e) => {
                                eprintln!("[remote-control] recv_envelope: dropping undecodable plaintext frame: {e}");
                                continue;
                            }
                        }
                    } else {
                        let enc: EncryptedEnvelope = serde_json::from_str(&frame)
                            .context("decode encrypted envelope")?;
                        match decrypt(key, &enc) {
                            Ok(env) => env,
                            Err(e) => {
                                // Decrypt failure = the peer re-paired with a
                                // fresh key and this connection's key is
                                // stale. Drop the key so the next envelope
                                // is treated as plaintext and the caller's
                                // reconnect can re-pair cleanly.
                                eprintln!("[remote-control] recv_envelope: decrypt failed ({e}); dropping stale session key");
                                self.session_key = None;
                                self.peer_device_id = None;
                                return Err(anyhow::anyhow!("decrypt failed: {e}"));
                            }
                        }
                    }
                }
                None => {
                    if frame.contains("\"to_device_id\"") {
                        eprintln!("[remote-control] recv_envelope: skipping encrypted frame before key installed");
                        continue;
                    }
                    serde_json::from_str(&frame).context("decode plain envelope")?
                }
            };
            return Ok(Some(envelope));
        }
    }

    /// Send a pre-serialized plaintext frame (heartbeat). Bypasses E2E
    /// encryption intentionally: the relay must see the frame to keep the
    /// connection alive, and encrypted envelopes are routed to the peer
    /// rather than consumed by the relay.
    pub async fn send_heartbeat(&mut self, frame: &str) -> Result<()> {
        self.transport.send_text(frame.to_string()).await
    }

    /// Pre-pairing auth: send a `DeviceAuth` envelope (plain) and await an
    /// ack. Must be called before `install_session_key`. `device_pubkey` is
    /// the Ed25519 verifying key (base64url-no-pad) the relay uses to verify
    /// `signature`; pass `None` only for tests/legacy peers.
    pub async fn authenticate(
        &mut self,
        device_id: &str,
        device_pubkey: Option<&str>,
        signature: &str,
        timestamp_ms: u64,
    ) -> Result<()> {
        let auth = Message::DeviceAuth(relay_protocol::DeviceAuth {
            device_id: device_id.to_string(),
            device_pubkey: device_pubkey.map(|s| s.to_string()),
            signature: signature.to_string(),
            timestamp_ms,
        });
        let envelope = Envelope::from_message(None, &auth)?;
        self.send_envelope(&envelope).await?;
        let ack = self
            .recv_envelope()
            .await?
            .context("relay closed during auth handshake")?;
        match ack.into_message()? {
            Message::DeviceAuth(_) | Message::SubscriptionStatus(_) => Ok(()),
            other => Err(anyhow::anyhow!("unexpected auth response: {other:?}")),
        }
    }

    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
    }

    pub async fn close(&mut self) {
        self.transport.close().await;
    }
}

/// Spawn a mock relay that parses each inbound `Envelope` (plain) and calls
/// `on_envelope` to decide the reply. Used for auth + plaintext routing
/// tests. Returns the `ws://127.0.0.1:PORT` URL to dial.
pub async fn spawn_mock_relay<F>(on_envelope: F) -> Result<String>
where
    F: Fn(Envelope) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Envelope>> + Send>>
        + Send
        + Sync
        + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let url = format!("ws://127.0.0.1:{port}");
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                let mut server = WsTransport::new(ws);
                while let Ok(Some(frame)) = server.recv_text().await {
                    let Ok(envelope) = serde_json::from_str::<Envelope>(&frame) else {
                        break;
                    };
                    match on_envelope(envelope).await {
                        Some(reply) => {
                            let json = serde_json::to_string(&reply).unwrap();
                            if server.send_text(json).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                server.close().await;
            }
        }
    });
    Ok(url)
}

/// Spawn a passthrough mock relay that forwards every raw text frame back to
/// the client unchanged. Used for E2E driver tests where both endpoints
/// encrypt/decrypt and the relay must not inspect payloads. Returns the
/// `ws://127.0.0.1:PORT` URL to dial.
pub async fn spawn_passthrough_relay() -> Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let url = format!("ws://127.0.0.1:{port}");
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                let mut server = WsTransport::new(ws);
                while let Ok(Some(frame)) = server.recv_text().await {
                    if server.send_text(frame).await.is_err() {
                        break;
                    }
                }
                server.close().await;
            }
        }
    });
    Ok(url)
}

/// Dial a (plain ws://) endpoint. The real client uses `connect_async` with
/// TLS; tests use this against the mock relays.
///
/// `insecure` skips TLS certificate verification (for self-signed relay
/// hosts during development). It has no effect on plain `ws://` URLs and
/// MUST be gated behind an explicit opt-in by the caller — never default
/// to `true`.
pub async fn dial_plain(
    url: &str,
    heartbeat: Duration,
    insecure: bool,
) -> Result<RelayConnection<WsTransport<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>>
{
    let (ws, _response) = if insecure {
        dial_tls_insecure(url).await.context("dial relay (insecure TLS)")?
    } else {
        tokio_tungstenite::connect_async(url)
            .await
            .context("dial relay")?
    };
    Ok(RelayConnection::new(WsTransport::new(ws), heartbeat))
}

/// Dial a `wss://` endpoint with a rustls config that accepts any server
/// certificate. Used only when the caller has explicitly opted into
/// insecure TLS (e.g. a self-signed relay host in development). The
/// returned stream type matches `connect_async` so callers stay uniform.
async fn dial_tls_insecure(
    url: &str,
) -> Result<(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::handshake::client::Response,
)> {
    use std::sync::Arc;

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifyServerCert))
        .with_no_client_auth();
    tokio_tungstenite::connect_async_tls_with_config(
        url,
        None,
        false,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await
    .context("dial relay (insecure TLS)")
}

/// A `ServerCertVerifier` that accepts every certificate chain and
/// signature without checking anything. **Only safe for debugging against
/// a self-signed host you control.** Gated behind `dial_tls_insecure`.
#[derive(Debug)]
struct NoVerifyServerCert;

impl rustls::client::danger::ServerCertVerifier for NoVerifyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        // Delegate to rustls's default supported schemes so the handshake
        // can negotiate a cipher suite; we just skip the actual checks.
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
            .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_protocol::{ControlRequest, Message};
    use uuid::Uuid;

    #[tokio::test]
    async fn mock_relay_echoes_envelope_roundtrip() {
        let url = spawn_mock_relay(|envelope| Box::pin(async move { Some(envelope) }))
            .await
            .unwrap();
        let mut conn = dial_plain(&url, Duration::from_secs(30), false).await.unwrap();

        let request_id = Uuid::new_v4();
        let envelope = Envelope::from_message(
            Some(request_id),
            &Message::ControlRequest(ControlRequest::Cancel { request_id }),
        )
        .unwrap();
        conn.send_envelope(&envelope).await.unwrap();

        let received = conn
            .recv_envelope()
            .await
            .expect("recv ok")
            .expect("envelope echoed");
        assert_eq!(received, envelope);
        conn.close().await;
    }

    #[tokio::test]
    async fn authenticate_handshake_succeeds_against_mock_relay() {
        use relay_protocol::{DeviceAuth, Message};
        let url = spawn_mock_relay(|envelope| {
            Box::pin(async move {
                if matches!(envelope.into_message().ok(), Some(Message::DeviceAuth(_))) {
                    Some(
                        Envelope::from_message(
            None,
            &Message::DeviceAuth(DeviceAuth {
                device_id: "relay-ack".to_string(),
                device_pubkey: None,
                signature: String::new(),
                timestamp_ms: 0,
            }),
        )
        .unwrap(),
                    )
                } else {
                    None
                }
            })
        })
        .await
        .unwrap();
        let mut conn = dial_plain(&url, Duration::from_secs(30), false).await.unwrap();
        conn.authenticate("dev-pc", None, "sig-b64", 1_700_000_000_000)
            .await
            .expect("auth handshake succeeds");
        conn.close().await;
    }

    #[tokio::test]
    async fn recv_returns_none_on_clean_close() {
        let url = spawn_mock_relay(|_| Box::pin(async move { None })).await.unwrap();
        let mut conn = dial_plain(&url, Duration::from_secs(30), false).await.unwrap();
        let request_id = Uuid::new_v4();
        let envelope = Envelope::from_message(
            Some(request_id),
            &Message::ControlRequest(ControlRequest::Cancel { request_id }),
        )
        .unwrap();
        conn.send_envelope(&envelope).await.unwrap();
        let received = conn.recv_envelope().await.unwrap();
        assert!(received.is_none(), "clean close yields None");
    }

    #[tokio::test]
    async fn e2e_envelope_roundtrips_through_passthrough_relay() {
        // Both endpoints share a session key; the relay forwards ciphertext
        // unchanged. Proves encrypt -> relay -> decrypt recovers the envelope.
        let url = spawn_passthrough_relay().await.unwrap();
        let mut conn = dial_plain(&url, Duration::from_secs(30), false).await.unwrap();
        let key = SessionKey::derive(b"pairing-secret", b"kodex-relay-salt");
        conn.install_session_key(key.clone(), "dev-phone".to_string());

        let request_id = Uuid::new_v4();
        let envelope = Envelope::from_message(
            Some(request_id),
            &Message::ControlRequest(ControlRequest::Cancel { request_id }),
        )
        .unwrap();
        conn.send_envelope(&envelope).await.unwrap();
        let received = conn
            .recv_envelope()
            .await
            .expect("recv ok")
            .expect("envelope recovered");
        assert_eq!(received, envelope);
        conn.close().await;
    }

    #[tokio::test]
    async fn plaintext_frame_accepted_after_session_key_installed() {
        // The resume flow: PC holds an E2E key from the earlier pairing, the
        // phone resumes, and the relay forwards a *plaintext* PairingConfirm
        // with fresh material. The keyed connection must parse it instead of
        // failing with `decode encrypted envelope: missing field
        // to_device_id` and tearing the driver down.
        let url = spawn_passthrough_relay().await.unwrap();
        let mut conn = dial_plain(&url, Duration::from_secs(30), false).await.unwrap();
        // First frame goes out plaintext (no key yet), echoes back plaintext.
        let heartbeat = Envelope::from_message(None, &Message::Heartbeat).unwrap();
        conn.send_envelope(&heartbeat).await.unwrap();
        // Now install a key: outbound frames would encrypt, but the echoed
        // plaintext heartbeat must still parse.
        let key = SessionKey::derive(b"old-secret", b"kodex-relay-salt");
        conn.install_session_key(key, "dev-phone".to_string());
        let echoed = conn
            .recv_envelope()
            .await
            .expect("plaintext frame parses with a key installed")
            .expect("echoed frame");
        assert_eq!(echoed, heartbeat);
        conn.close().await;
    }
}
