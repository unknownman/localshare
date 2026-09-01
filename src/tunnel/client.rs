//! Tunnel connection coordinator and lifecycle manager.
//!
//! The public entry point is [`run_tunnel`], which drives an outer reconnect
//! loop and an inner per-connection session. Callers receive lifecycle and
//! traffic events through a [`tokio::sync::broadcast`] channel.
//!
//! # Architecture
//!
//! ```text
//!  run_tunnel()
//!    └─ connect_once()                ← handshake + session loop
//!         ├─ heartbeat task           ← periodic ClientPing / Pong watchdog
//!         ├─ ws_sink task             ← drains response_tx → WebSocket
//!         └─ dispatch loop            ← routes relay messages
//!               ├─ RequestStart  → spawn process_stream() task
//!               ├─ RequestChunk  → route to stream channel
//!               ├─ RequestEnd    → route to stream channel
//!               ├─ StreamReset   → send Cancel to stream channel
//!               └─ RelayPong     → refresh heartbeat deadline
//! ```

use crate::error::RelayError;
use crate::tunnel::forward::process_stream;
use crate::tunnel::protocol::{
    parse_relay, LocalTarget, Message, Payload, RelayEndpoint, StreamMessage,
};
use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata about an active, established tunnel session.
#[derive(Debug, Clone)]
pub struct TunnelSession {
    pub subdomain: String,
    pub public_url: String,
    pub heartbeat_interval_ms: u64,
}

/// Events emitted by the tunnel runner that UI or scripting layers can observe.
#[derive(Debug, Clone)]
pub enum TunnelEvent {
    /// Attempting to connect (or reconnect) to the relay.
    Connecting { endpoint: String },
    /// Successfully registered; the public URL is now live.
    Connected { session: TunnelSession },
    /// Connection dropped; about to retry after `delay`.
    Reconnecting { attempt: u32, delay: Duration },
    /// A request was fully proxied.
    RequestHandled {
        stream_id: u64,
        method: String,
        path: String,
        status: u16,
        duration: Duration,
        /// Optional user-facing hint when the request failed (e.g. a local
        /// connection refused). `None` for successful requests.
        hint: Option<String>,
    },
    /// The tunnel was disconnected, either because the process is shutting
    /// down (`graceful: true`), the relay closed the connection cleanly
    /// (`graceful: true`), or because of a fatal error (`graceful: false`).
    /// Graceful disconnects exit `0`; fatal ones exit non-zero.
    Disconnected { reason: String, graceful: bool },
}

/// Configuration for [`run_tunnel`].
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    /// Relay address (bare hostname, host:port, or ws(s):// URL).
    pub relay: String,
    /// Optional subdomain to request from the relay.
    pub requested_subdomain: Option<String>,
    /// Human-readable client name sent in the `Register` message.
    pub client_name: String,
    /// Client version string sent in the `Register` message.
    pub client_version: String,
    /// The local HTTP server to forward traffic to.
    pub target: LocalTarget,
    /// Timeout for the initial registration handshake.
    pub handshake_timeout: Duration,
    /// How long to wait for a Pong before declaring the connection dead.
    pub pong_timeout: Duration,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            relay: "relay.localshare.dev".into(),
            requested_subdomain: None,
            client_name: "localshare".into(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            target: LocalTarget {
                host: "127.0.0.1".into(),
                port: 3000,
            },
            handshake_timeout: Duration::from_secs(5),
            pong_timeout: Duration::from_secs(30),
        }
    }
}

// ── Reconnect constants ───────────────────────────────────────────────────────

const BACKOFF_BASE_MS: u64 = 500;
const BACKOFF_FACTOR: f64 = 1.5;
const BACKOFF_MAX_MS: u64 = 15_000;
/// Jitter is applied as ±JITTER_FRACTION of the computed delay.
const JITTER_FRACTION: f64 = 0.25;

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the tunnel with automatic reconnection until `cancel` is triggered.
///
/// Returns a broadcast receiver that delivers `TunnelEvent`s. The channel
/// capacity is 256; slow receivers will miss events rather than block the
/// tunnel.
pub async fn run_tunnel(
    config: TunnelConfig,
    cancel: CancellationToken,
) -> broadcast::Receiver<TunnelEvent> {
    let (event_tx, event_rx) = broadcast::channel::<TunnelEvent>(256);

    let endpoint = match parse_relay(&config.relay) {
        Ok(ep) => ep,
        Err(e) => {
            let _ = event_tx.send(TunnelEvent::Disconnected {
                reason: e.to_string(),
                graceful: false,
            });
            return event_rx;
        }
    };

    tokio::spawn(reconnect_loop(config, endpoint, event_tx, cancel));
    event_rx
}

// ── Reconnect loop ────────────────────────────────────────────────────────────

async fn reconnect_loop(
    config: TunnelConfig,
    endpoint: RelayEndpoint,
    event_tx: broadcast::Sender<TunnelEvent>,
    cancel: CancellationToken,
) {
    let mut attempt: u32 = 0;
    let mut last_subdomain: Option<String> = config.requested_subdomain.clone();
    let mut ever_connected = false;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let _ = event_tx.send(TunnelEvent::Connecting {
            endpoint: endpoint.to_string(),
        });

        match connect_once(
            &config,
            &endpoint,
            last_subdomain.clone(),
            &event_tx,
            &cancel,
        )
        .await
        {
            Ok(assigned) => {
                // Successful session ended (graceful or cancelled).
                ever_connected = true;
                last_subdomain = Some(assigned);
                attempt = 0;
                if cancel.is_cancelled() {
                    break;
                }
            }
            Err(e) => {
                let reason = e.to_string();
                tracing::warn!(attempt, error = %reason, "relay connection lost");

                // A connection that fails before ever succeeding is treated as
                // fatal: there is no tunnel to repair, so retrying indefinitely
                // would just spin. Emit a clean Disconnected and stop. Unless a
                // shutdown has been requested meanwhile (e.g. Ctrl+C during the
                // handshake), which should always be a graceful exit 0.
                if cancel.is_cancelled() {
                    break;
                }
                if !ever_connected {
                    let _ = event_tx.send(TunnelEvent::Disconnected {
                        reason,
                        graceful: false,
                    });
                    break;
                }

                // The relay closed the connection cleanly (a WebSocket close
                // frame or an orderly EOF), e.g. during a server restart. That
                // is a graceful end-of-session: report it and exit 0 rather
                // than retrying or panicking.
                if is_graceful_close(&e) {
                    let _ = event_tx.send(TunnelEvent::Disconnected {
                        reason: "The relay closed the connection.".into(),
                        graceful: true,
                    });
                    break;
                }

                if cancel.is_cancelled() {
                    break;
                }

                // Exponential back-off with jitter.
                let base = (BACKOFF_BASE_MS as f64 * BACKOFF_FACTOR.powi(attempt.min(20) as i32))
                    .min(BACKOFF_MAX_MS as f64);
                let jitter = base * JITTER_FRACTION;
                let delay_ms = {
                    use rand::Rng;
                    let delta = rand::thread_rng().gen_range(-jitter..=jitter);
                    ((base + delta).round() as u64).clamp(0, BACKOFF_MAX_MS)
                };
                let delay = Duration::from_millis(delay_ms);

                attempt = attempt.saturating_add(1);
                let _ = event_tx.send(TunnelEvent::Reconnecting { attempt, delay });

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => break,
                }
            }
        }
    }

    if cancel.is_cancelled() {
        let _ = event_tx.send(TunnelEvent::Disconnected {
            reason: "cancelled".into(),
            graceful: true,
        });
    }
}

/// Returns `true` when the error is a clean close from the relay (a WebSocket
/// close frame or an orderly EOF) rather than a transport-level failure that
/// warrants a reconnect attempt.
fn is_graceful_close(e: &RelayError) -> bool {
    matches!(e, RelayError::UnexpectedClose(_))
}

// ── Single connection lifecycle ───────────────────────────────────────────────

/// Establish one WebSocket connection, complete the registration handshake,
/// and run the session until it ends (gracefully or with an error).
///
/// Returns `Ok(subdomain)` on clean exit so the caller can preserve it for
/// the next reconnect attempt.
async fn connect_once(
    config: &TunnelConfig,
    endpoint: &RelayEndpoint,
    requested_subdomain: Option<String>,
    event_tx: &broadcast::Sender<TunnelEvent>,
    cancel: &CancellationToken,
) -> Result<String, RelayError> {
    let url = endpoint.ws_url();
    tracing::debug!(url, "connecting to relay");

    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| RelayError::Connect(url.clone(), e.to_string()))?;

    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // ── 1. Send Register ──────────────────────────────────────────────────────
    let register_msg = Message::new(Payload::Register {
        requested_subdomain: requested_subdomain.clone(),
        client_name: config.client_name.clone(),
        client_version: config.client_version.clone(),
        heartbeat_interval_ms: 15_000,
    });
    ws_sink
        .send(register_msg.into_ws_message())
        .await
        .map_err(|e| RelayError::Connect(url.clone(), e.to_string()))?;

    // ── 2. Await Registered / RegisterRejected with timeout ───────────────────
    let session = tokio::time::timeout(config.handshake_timeout, async {
        while let Some(frame) = ws_stream.next().await {
            let frame = frame.map_err(|e| RelayError::Connect(url.clone(), e.to_string()))?;
            let msg = match Message::from_ws_message(&frame) {
                Ok(m) => m,
                Err(_) => continue, // ignore unparseable frames during handshake
            };
            match msg.payload {
                Payload::Registered {
                    subdomain,
                    public_url,
                    heartbeat_interval_ms,
                } => {
                    return Ok(TunnelSession {
                        subdomain,
                        public_url,
                        heartbeat_interval_ms,
                    });
                }
                Payload::RegisterRejected { reason } => {
                    return Err(RelayError::RegistrationRejected(reason));
                }
                _ => continue,
            }
        }
        Err(RelayError::UnexpectedClose(
            "relay closed during handshake".into(),
        ))
    })
    .await
    .map_err(|_| RelayError::HandshakeTimeout(endpoint.to_string(), config.handshake_timeout))??;

    let subdomain = session.subdomain.clone();
    let heartbeat_ms = session.heartbeat_interval_ms;
    let _ = event_tx.send(TunnelEvent::Connected { session });

    // ── 3. Channels shared between the session tasks ───────────────────────────
    // response_tx: forwarding tasks → ws_sink_task (→ relay)
    let (response_tx, response_rx) = mpsc::channel::<tungstenite::Message>(256);
    // active_streams: stream_id → sender end of per-stream channel
    let mut active_streams: HashMap<u64, mpsc::Sender<StreamMessage>> = HashMap::new();
    // Track heartbeat deadline.
    let pong_deadline = tokio::time::Instant::now() + config.pong_timeout;
    let pong_deadline = std::sync::Arc::new(std::sync::Mutex::new(pong_deadline));

    // ── 4. Spawn WebSocket sink writer task ───────────────────────────────────
    // The task runs until every `response_tx` sender is dropped: on shutdown
    // the session loop enqueues a final `Unregister` frame, and this task must
    // flush it (and a Close frame) to the relay before the process exits.
    let sink_handle = tokio::spawn(async move {
        let mut response_rx = response_rx;
        while let Some(m) = response_rx.recv().await {
            if ws_sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = ws_sink.close().await;
    });

    // ── 5. Spawn heartbeat task ───────────────────────────────────────────────
    let heartbeat_response_tx = response_tx.clone();
    let heartbeat_cancel = cancel.clone();
    let heartbeat_pong_deadline = pong_deadline.clone();
    let pong_timeout = config.pong_timeout;
    tokio::spawn(async move {
        let interval = Duration::from_millis(heartbeat_ms);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = heartbeat_cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let ping = Message::new(Payload::ClientPing).into_ws_message();
                    if heartbeat_response_tx.send(ping).await.is_err() {
                        break;
                    }
                    // Check if pong deadline has been missed.
                    let deadline = *heartbeat_pong_deadline
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if tokio::time::Instant::now() > deadline + pong_timeout {
                        tracing::warn!("relay pong not received within deadline; treating as dead");
                        heartbeat_cancel.cancel();
                        break;
                    }
                }
            }
        }
    });

    // ── 6. Main dispatch loop ─────────────────────────────────────────────────
    let mut error: Option<RelayError> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            frame = ws_stream.next() => {
                let frame = match frame {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => {
                        error = Some(RelayError::Connect(url.clone(), e.to_string()));
                        break;
                    }
                    None => {
                        error = Some(RelayError::UnexpectedClose("relay closed the connection".into()));
                        break;
                    }
                };

                // Skip WebSocket-level control frames.
                if matches!(
                    frame,
                    tungstenite::Message::Ping(_)
                    | tungstenite::Message::Pong(_)
                    | tungstenite::Message::Close(_)
                ) {
                    // Update pong deadline on any Pong frame.
                    if matches!(frame, tungstenite::Message::Pong(_)) {
                        let mut dl = pong_deadline
                            .lock()
                            .unwrap_or_else(|p| p.into_inner());
                        *dl = tokio::time::Instant::now() + config.pong_timeout;
                    }
                    continue;
                }

                let msg = match Message::from_ws_message(&frame) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to parse relay message; skipping");
                        continue;
                    }
                };

                dispatch_message(
                    msg.payload,
                    &config.target,
                    &mut active_streams,
                    &response_tx,
                    event_tx,
                    &pong_deadline,
                    config.pong_timeout,
                )
                .await;
            }
        }
    }

    // ── 7. Graceful shutdown ──────────────────────────────────────────────────
    // Send Unregister best-effort.
    let unregister = Message::new(Payload::Unregister { reason: None }).into_ws_message();
    let _ = tokio::time::timeout(Duration::from_millis(500), response_tx.send(unregister)).await;

    // Signal all active stream workers to stop.
    for (_, tx) in active_streams.drain() {
        let _ = tx.send(StreamMessage::Cancel).await;
    }

    // Allow up to 2 seconds for in-flight streams to drain.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Drop the last channel sender we own, then wait for the sink to flush the
    // final `Unregister` and complete its Close handshake. Awaiting the sink
    // here guarantees the WebSocket is fully closed before the caller lets the
    // process exit, so the relay never observes a rude TCP reset.
    drop(response_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), sink_handle).await;

    match error {
        Some(e) => Err(e),
        None => Ok(subdomain),
    }
}

// ── Message dispatcher ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn dispatch_message(
    payload: Payload,
    target: &LocalTarget,
    active_streams: &mut HashMap<u64, mpsc::Sender<StreamMessage>>,
    response_tx: &mpsc::Sender<tungstenite::Message>,
    event_tx: &broadcast::Sender<TunnelEvent>,
    pong_deadline: &std::sync::Arc<std::sync::Mutex<tokio::time::Instant>>,
    pong_timeout: Duration,
) {
    match payload {
        // ── New request ───────────────────────────────────────────────────────
        Payload::RequestStart {
            stream_id,
            method,
            path,
            headers,
        } => {
            let (stream_tx, stream_rx) = mpsc::channel::<StreamMessage>(64);
            active_streams.insert(stream_id, stream_tx);

            // Wrap the message sender so forwarding tasks produce `tungstenite::Message`.
            let (msg_tx, mut msg_rx) = mpsc::channel::<Message>(64);
            let ws_tx = response_tx.clone();
            let event_tx_clone = event_tx.clone();
            let start = std::time::Instant::now();
            let method_clone = method.clone();
            let path_clone = path.clone();
            let target_host = target.host.clone();
            let target_port = target.port;
            tokio::spawn(async move {
                let mut captured_status: Option<u16> = None;
                let mut hint: Option<String> = None;
                let mut emitted = false;
                while let Some(m) = msg_rx.recv().await {
                    match &m.payload {
                        Payload::ResponseStart { status_code, .. } => {
                            captured_status = Some(*status_code);
                        }
                        Payload::ResponseError { code, .. } => {
                            let status = match code {
                                crate::tunnel::protocol::ResponseErrorCode::TargetConnectionRefused => 502,
                                crate::tunnel::protocol::ResponseErrorCode::TargetTimeout => 504,
                                crate::tunnel::protocol::ResponseErrorCode::LocalIoError => 502,
                            };
                            captured_status = Some(status);
                            if *code
                                == crate::tunnel::protocol::ResponseErrorCode::TargetConnectionRefused
                            {
                                hint = Some(format!(
                                    "Is your local server running on {}:{}?",
                                    target_host, target_port
                                ));
                            }
                            if !emitted {
                                emitted = true;
                                let _ = event_tx_clone.send(TunnelEvent::RequestHandled {
                                    stream_id,
                                    method: method_clone.clone(),
                                    path: path_clone.clone(),
                                    status,
                                    duration: start.elapsed(),
                                    hint: hint.clone(),
                                });
                            }
                        }
                        Payload::ResponseEnd { .. } if !emitted => {
                            emitted = true;
                            let status = captured_status.unwrap_or(200);
                            let _ = event_tx_clone.send(TunnelEvent::RequestHandled {
                                stream_id,
                                method: method_clone.clone(),
                                path: path_clone.clone(),
                                status,
                                duration: start.elapsed(),
                                hint: hint.clone(),
                            });
                        }
                        _ => {}
                    }
                    let _ = ws_tx.send(m.into_ws_message()).await;
                }
            });

            process_stream(
                stream_id,
                method,
                path,
                headers,
                target.clone(),
                stream_rx,
                msg_tx,
            );
        }

        // ── Ongoing request body ──────────────────────────────────────────────
        Payload::RequestChunk { stream_id, data } => {
            if let Some(tx) = active_streams.get(&stream_id) {
                let _ = tx
                    .send(StreamMessage::Payload(Payload::RequestChunk {
                        stream_id,
                        data,
                    }))
                    .await;
            } else {
                tracing::debug!(stream_id, "RequestChunk for unknown stream; ignoring");
            }
        }

        Payload::RequestEnd { stream_id } => {
            if let Some(tx) = active_streams.get(&stream_id) {
                let _ = tx
                    .send(StreamMessage::Payload(Payload::RequestEnd { stream_id }))
                    .await;
            }
        }

        // ── Relay asks us to abort a stream ───────────────────────────────────
        Payload::StreamReset { stream_id } => {
            if let Some(tx) = active_streams.remove(&stream_id) {
                let _ = tx.send(StreamMessage::Cancel).await;
            }
        }

        // ── Heartbeat response ────────────────────────────────────────────────
        Payload::RelayPong => {
            let mut dl = pong_deadline.lock().unwrap_or_else(|p| p.into_inner());
            *dl = tokio::time::Instant::now() + pong_timeout;
        }

        other => {
            tracing::debug!(payload = ?other, "unhandled relay payload in dispatch");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::protocol::{Message, Payload, RejectReason};
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    // Helper: create a minimal mock relay WebSocket server and return its port.
    async fn spawn_mock_relay(response: Payload) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(tcp).await.unwrap();

            // Read Register message.
            let _register_frame = ws.next().await.unwrap().unwrap();

            // Send the configured response.
            let resp_bytes = serde_json::to_vec(&Message::new(response)).unwrap();
            ws.send(tungstenite::Message::Binary(resp_bytes))
                .await
                .unwrap();

            // Keep the connection open so the client has time to process it.
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        port
    }

    // ── Registration handshake ────────────────────────────────────────────────

    #[tokio::test]
    async fn successful_registration_emits_connected_event() {
        let port = spawn_mock_relay(Payload::Registered {
            subdomain: "test-sub".into(),
            public_url: "https://test-sub.relay.localshare.dev".into(),
            heartbeat_interval_ms: 30_000,
        })
        .await;

        let cancel = CancellationToken::new();
        let config = TunnelConfig {
            relay: format!("ws://127.0.0.1:{}", port),
            client_name: "test".into(),
            client_version: "0.0.1".into(),
            target: LocalTarget {
                host: "127.0.0.1".into(),
                port: 9999,
            },
            handshake_timeout: Duration::from_secs(5),
            pong_timeout: Duration::from_secs(30),
            ..Default::default()
        };

        let mut rx = run_tunnel(config, cancel.clone()).await;

        // Drain events until we see Connected or a non-Connecting event.
        let event = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(TunnelEvent::Connecting { .. }) => continue,
                    Ok(e) => return e,
                    Err(_) => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("timed out waiting for Connected event");

        match event {
            TunnelEvent::Connected { session } => {
                assert_eq!(session.subdomain, "test-sub");
                assert_eq!(session.public_url, "https://test-sub.relay.localshare.dev");
            }
            other => panic!("expected Connected, got {:?}", other),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn registration_rejection_emits_disconnected() {
        let port = spawn_mock_relay(Payload::RegisterRejected {
            reason: RejectReason::SubdomainTaken,
        })
        .await;

        let cancel = CancellationToken::new();
        let config = TunnelConfig {
            relay: format!("ws://127.0.0.1:{}", port),
            client_name: "test".into(),
            client_version: "0.0.1".into(),
            target: LocalTarget {
                host: "127.0.0.1".into(),
                port: 9999,
            },
            handshake_timeout: Duration::from_secs(5),
            pong_timeout: Duration::from_secs(30),
            ..Default::default()
        };

        let mut rx = run_tunnel(config, cancel.clone()).await;

        // A rejected registration before any successful connection is fatal:
        // we expect a Disconnected event signalling a graceful stop.
        let got_disconnected = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(TunnelEvent::Disconnected { .. }) => return true,
                    Ok(_) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await
        .expect("timed out");

        assert!(
            got_disconnected,
            "expected Disconnected event after rejection"
        );
        cancel.cancel();
    }
}
