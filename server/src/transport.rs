use std::net::SocketAddr;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite;

use crate::errors::Result;
use crate::session::Session;
use crate::state::AppState;

/// Accept loop: one task per WebSocket connection.
pub async fn run(state: AppState) -> Result<()> {
    let listener = TcpListener::bind(state.config.listen_addr).await?;
    tracing::info!(
        addr = %state.config.listen_addr,
        require_tls = state.config.require_tls,
        "relay server listening (plain ws; terminate TLS at a reverse proxy for production wss://)"
    );
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, peer, state).await {
                        tracing::warn!(%peer, error = %e, "connection ended");
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "accept failed"),
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    _peer: SocketAddr,
    state: AppState,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (ws_writer, ws_reader) = ws.split();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    // Send protocol-level WebSocket pings periodically so idle peers stay
    // alive. The read loop below treats *any* inbound frame as activity, so a
    // peer's automatic Pong response resets its 60s heartbeat timeout even
    // when no application traffic is flowing.
    let ping_interval = Duration::from_secs((state.config.heartbeat_timeout_secs / 3).max(2));
    let mut ping = tokio::time::interval(ping_interval);

    let writer = tokio::spawn(async move {
        let mut rx = rx;
        let mut ws_writer = ws_writer;
        loop {
            tokio::select! {
                maybe_text = rx.recv() => {
                    match maybe_text {
                        Some(text) => {
                            if ws_writer
                                .send(tungstenite::Message::Text(text.into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = ping.tick() => {
                    if ws_writer
                        .send(tungstenite::Message::Ping(vec![].into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            if rx.is_closed() {
                break;
            }
        }
        let _ = ws_writer.send(tungstenite::Message::Close(None)).await;
    });

    let mut session = Session::new(state, tx);
    session.run(ws_reader).await;
    drop(session);
    let _ = writer.await;
    Ok(())
}
