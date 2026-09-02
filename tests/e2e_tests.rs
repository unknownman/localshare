//! End-to-end integration tests that run the real `localshare` binary against
//! a mocked local HTTP server and a mocked relay WebSocket server.
//!
//! These are the closest thing to a production smoke test: they validate the
//! full wire protocol across the binary boundary, not just in-process units.

use futures_util::{SinkExt, StreamExt};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::accept_async;

// ── Minimal on-the-wire protocol types ─────────────────────────────────────────
// Duplicated here intentionally: this test validates interop with the real
// binary over the wire, so it must not import the crate's internals.

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseErrorCode {
    TargetConnectionRefused,
    TargetTimeout,
    LocalIoError,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Payload {
    Register {
        requested_subdomain: Option<String>,
        client_name: String,
        client_version: String,
        heartbeat_interval_ms: u64,
    },
    ClientPing,
    ResponseStart {
        stream_id: u64,
        status_code: u16,
        headers: Vec<Header>,
    },
    ResponseChunk {
        stream_id: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    ResponseEnd {
        stream_id: u64,
    },
    ResponseError {
        stream_id: u64,
        code: ResponseErrorCode,
        message: String,
    },
    Unregister {
        reason: Option<String>,
    },
    Registered {
        subdomain: String,
        public_url: String,
        heartbeat_interval_ms: u64,
    },
    RelayPong,
    RequestStart {
        stream_id: u64,
        method: String,
        path: String,
        headers: Vec<Header>,
    },
    RequestEnd {
        stream_id: u64,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Message {
    version: u8,
    payload: Payload,
}

impl Message {
    fn new(payload: Payload) -> Self {
        Self {
            version: 1,
            payload,
        }
    }

    fn to_binary(&self) -> tungstenite::Message {
        tungstenite::Message::Binary(serde_json::to_vec(self).expect("Message serialization"))
    }

    fn from_ws(msg: &tungstenite::Message) -> Option<Self> {
        let bytes: &[u8] = match msg {
            tungstenite::Message::Text(s) => s.as_bytes(),
            tungstenite::Message::Binary(b) => b.as_ref(),
            _ => return None,
        };
        serde_json::from_slice(bytes).ok()
    }
}

// ── Mock relay outcome ─────────────────────────────────────────────────────────

#[derive(Debug)]
enum RelayOutcome {
    Served { status: u16, body: Vec<u8> },
    Refused { message: String },
}

/// Spawn a mock relay that accepts one client, completes the registration
/// handshake, proxies a single GET request, and reports what the client sent
/// back over `oneshot`.
///
/// When `expect_refused` is true the relay drives the same request but expects
/// the client to reply with `ResponseError(TargetConnectionRefused)` instead
/// of an actual HTTP response (the local server is not running).
async fn spawn_mock_relay(expect_refused: bool) -> (u16, oneshot::Receiver<RelayOutcome>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let port = listener.local_addr().expect("relay addr").port();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept client");
        let mut ws = accept_async(tcp).await.expect("ws handshake");

        // Expect the Register message first.
        let first = ws
            .next()
            .await
            .expect("client should send Register")
            .expect("frame ok");
        let msg = Message::from_ws(&first).expect("first frame should parse");
        assert!(
            matches!(msg.payload, Payload::Register { .. }),
            "first client message must be Register"
        );

        // Accept the registration.
        ws.send(
            Message::new(Payload::Registered {
                subdomain: "e2e-test".into(),
                public_url: "https://e2e-test.localshare.dev".into(),
                heartbeat_interval_ms: 60_000,
            })
            .to_binary(),
        )
        .await
        .expect("send Registered");

        // Proxy one GET request.
        ws.send(
            Message::new(Payload::RequestStart {
                stream_id: 1,
                method: "GET".into(),
                path: "/hello".into(),
                headers: vec![],
            })
            .to_binary(),
        )
        .await
        .expect("send RequestStart");
        ws.send(Message::new(Payload::RequestEnd { stream_id: 1 }).to_binary())
            .await
            .expect("send RequestEnd");

        // Read the client's responses until the scenario completes.
        let deadline = Duration::from_secs(15);
        let mut status: Option<u16> = None;
        let mut body = Vec::new();

        loop {
            let frame = tokio::time::timeout(deadline, ws.next())
                .await
                .expect("timed out waiting for client response")
                .expect("websocket closed")
                .expect("websocket error");

            let Some(msg) = Message::from_ws(&frame) else {
                continue;
            };
            match msg.payload {
                Payload::ClientPing => {
                    ws.send(Message::new(Payload::RelayPong).to_binary())
                        .await
                        .expect("send RelayPong");
                }
                Payload::ResponseStart { status_code, .. } => {
                    status = Some(status_code);
                }
                Payload::ResponseChunk { data, .. } => body.extend_from_slice(&data),
                Payload::ResponseEnd { .. } => {
                    let _ = tx.send(RelayOutcome::Served {
                        status: status.expect("ResponseStart must precede ResponseEnd"),
                        body,
                    });
                    return;
                }
                Payload::ResponseError { message, .. } if expect_refused => {
                    let _ = tx.send(RelayOutcome::Refused { message });
                    return;
                }
                _ => {}
            }
        }
    });

    (port, rx)
}

// ── Mock local HTTP server ─────────────────────────────────────────────────────

/// Spawn a minimal HTTP server that answers every request with
/// `200 OK` and the given body. Returns the bound port.
async fn spawn_mock_local(body: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind local");
    let port = listener.local_addr().expect("local addr").port();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept local req");
        let mut buf = vec![0u8; 4096];
        let mut received = Vec::new();
        // Read until the header boundary so we know the request arrived.
        loop {
            let n = socket.read(&mut buf).await.expect("read request");
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            if received.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).expect("utf8 body")
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
    });

    port
}

/// Spawn a minimal HTTP server that answers exactly `count` requests, in order,
/// with `200 OK` and the given body. Connections are served sequentially, which
/// matches how the client opens one connection per proxied request.
async fn spawn_mock_local_n(body: &'static [u8], count: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind local");
    let port = listener.local_addr().expect("local addr").port();

    tokio::spawn(async move {
        for _ in 0..count {
            let (mut socket, _) = listener.accept().await.expect("accept local req");
            let mut buf = vec![0u8; 4096];
            let mut received = Vec::new();
            loop {
                let n = socket.read(&mut buf).await.expect("read request");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                if received.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                body.len(),
                std::str::from_utf8(body).expect("utf8 body")
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });

    port
}

// ── Faulty-then-healthy local server ──────────────────────────────────────────

/// How the local server should misbehave on its *first* accepted connection.
#[derive(Debug, Clone, Copy)]
enum FaultBehavior {
    /// Accept, read the request head, then close without responding.
    DropAfterAccept,
    /// Accept, read the request head, then reply with bytes that are not a
    /// valid HTTP response before closing.
    MalformedResponse,
}

/// Spawn a local server that misbehaves on its first connection (per
/// `FaultBehavior`) and then serves a normal `200 OK` on the second. This lets
/// us assert that after a forwarding failure the tunnel keeps working for the
/// next request (no spin, no deadlock).
async fn spawn_faulty_then_healthy(behavior: FaultBehavior) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind local");
    let port = listener.local_addr().expect("local addr").port();

    tokio::spawn(async move {
        // First connection: read the request head, then follow the fault.
        let (mut socket, _) = listener.accept().await.expect("accept first");
        let mut buf = vec![0u8; 4096];
        let mut received = Vec::new();
        loop {
            let n = socket.read(&mut buf).await.expect("read first");
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            if received.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        match behavior {
            FaultBehavior::DropAfterAccept => drop(socket),
            FaultBehavior::MalformedResponse => {
                let _ = socket
                    .write_all(b"THIS IS NOT AN HTTP RESPONSE\r\n\r\n")
                    .await;
            }
        }

        // Second connection: healthy 200 OK.
        let (mut socket, _) = listener.accept().await.expect("accept second");
        let mut buf = vec![0u8; 4096];
        let mut received = Vec::new();
        loop {
            let n = socket.read(&mut buf).await.expect("read second");
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            if received.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let body = b"healthy after fault";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).expect("utf8 body")
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write second response");
    });

    port
}

// ── Relay that drives an error then a success ─────────────────────────────────

/// Spawn a mock relay that proxies *two* sequential GET requests. The first
/// request is sent to a locally-faulty server and must produce a `ResponseError`
/// (which the client synthesises into a 502); the second goes to the now-healthy
/// server and must come back as HTTP 200. Reporting both proves the forwarding
/// loop survives a failure and serves the next request.
async fn spawn_mock_relay_error_then_ok() -> (u16, oneshot::Receiver<(ResponseErrorCode, u16)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let port = listener.local_addr().expect("relay addr").port();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept client");
        let mut ws = accept_async(tcp).await.expect("ws handshake");
        let deadline = Duration::from_secs(15);

        // Registration handshake.
        let first = ws
            .next()
            .await
            .expect("client should send Register")
            .expect("frame ok");
        let msg = Message::from_ws(&first).expect("first frame should parse");
        assert!(matches!(msg.payload, Payload::Register { .. }));
        ws.send(
            Message::new(Payload::Registered {
                subdomain: "e2e-fault".into(),
                public_url: "https://e2e-fault.localshare.dev".into(),
                heartbeat_interval_ms: 60_000,
            })
            .to_binary(),
        )
        .await
        .expect("send Registered");

        // Request 1 → expect a ResponseError.
        ws.send(
            Message::new(Payload::RequestStart {
                stream_id: 1,
                method: "GET".into(),
                path: "/fault".into(),
                headers: vec![],
            })
            .to_binary(),
        )
        .await
        .expect("send RequestStart 1");
        ws.send(Message::new(Payload::RequestEnd { stream_id: 1 }).to_binary())
            .await
            .expect("send RequestEnd 1");

        let mut first_error: Option<ResponseErrorCode> = None;
        while first_error.is_none() {
            let frame = tokio::time::timeout(deadline, ws.next())
                .await
                .expect("timed out waiting for ResponseError")
                .expect("websocket closed before ResponseError")
                .expect("websocket error");
            let Some(msg) = Message::from_ws(&frame) else {
                continue;
            };
            match msg.payload {
                Payload::ClientPing => {
                    ws.send(Message::new(Payload::RelayPong).to_binary())
                        .await
                        .expect("send RelayPong");
                }
                Payload::ResponseError { code, .. } => first_error = Some(code),
                _ => {}
            }
        }

        // Request 2 → the tunnel must still be alive and forward a healthy 200.
        ws.send(
            Message::new(Payload::RequestStart {
                stream_id: 2,
                method: "GET".into(),
                path: "/ok".into(),
                headers: vec![],
            })
            .to_binary(),
        )
        .await
        .expect("send RequestStart 2");
        ws.send(Message::new(Payload::RequestEnd { stream_id: 2 }).to_binary())
            .await
            .expect("send RequestEnd 2");

        let mut status2: Option<u16> = None;
        while status2.is_none() {
            let frame = tokio::time::timeout(deadline, ws.next())
                .await
                .expect("timed out waiting for second response")
                .expect("websocket closed before second response")
                .expect("websocket error");
            let Some(msg) = Message::from_ws(&frame) else {
                continue;
            };
            match msg.payload {
                Payload::ClientPing => {
                    ws.send(Message::new(Payload::RelayPong).to_binary())
                        .await
                        .expect("send RelayPong");
                }
                Payload::ResponseStart { status_code, .. } => status2 = Some(status_code),
                _ => {}
            }
        }

        let _ = tx.send((
            first_error.expect("response error observed"),
            status2.expect("status2"),
        ));
    });

    (port, rx)
}

// ── Helpers to drive the binary ────────────────────────────────────────────────

fn spawn_child(relay_port: u16, target: u16) -> Child {
    let bin = assert_cmd::cargo::cargo_bin("localshare");
    Command::new(bin)
        .arg(target.to_string())
        .arg("--json")
        .arg("-r")
        .arg(format!("ws://127.0.0.1:{relay_port}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn localshare")
}

fn kill_and_drain(mut child: Child) -> String {
    let _ = child.kill();
    let _ = child.wait();

    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    out
}

/// Poll `try_wait` until the child exits or `timeout` elapses. Returns the
/// process exit code, or `None` if it never exited in time.
async fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<i32> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait on child") {
            return status.code();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── Shutdown (signal) test plumbing ────────────────────────────────────────────

/// Events reported back from the mock relay used by the signal-shutdown tests.
#[cfg(unix)]
#[derive(Debug)]
enum ShutdownRelayEvent {
    /// The client completed the registration handshake.
    Registered,
    /// The client sent `Unregister` after receiving a shutdown signal.
    UnregisterReceived,
}

/// Spawn a mock relay that completes the registration handshake, then waits to
/// observe the client's graceful `Unregister` message.
#[cfg(unix)]
async fn spawn_shutdown_relay() -> (u16, mpsc::UnboundedReceiver<ShutdownRelayEvent>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let port = listener.local_addr().expect("relay addr").port();
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept client");
        let mut ws = accept_async(tcp).await.expect("ws handshake");

        let first = ws
            .next()
            .await
            .expect("client should send Register")
            .expect("frame ok");
        let msg = Message::from_ws(&first).expect("first frame should parse");
        assert!(matches!(msg.payload, Payload::Register { .. }));

        ws.send(
            Message::new(Payload::Registered {
                subdomain: "e2e-shutdown".into(),
                public_url: "https://e2e-shutdown.localshare.dev".into(),
                heartbeat_interval_ms: 60_000,
            })
            .to_binary(),
        )
        .await
        .expect("send Registered");

        let _ = tx.send(ShutdownRelayEvent::Registered);

        // Wait (up to 15s) for the client to send Unregister after the signal.
        let deadline = Duration::from_secs(15);
        let got_unregister = tokio::time::timeout(deadline, async {
            loop {
                let frame = ws
                    .next()
                    .await
                    .expect("ws closed before Unregister arrived")
                    .expect("websocket error");
                if let Some(msg) = Message::from_ws(&frame) {
                    if matches!(msg.payload, Payload::Unregister { .. }) {
                        break;
                    }
                }
            }
        })
        .await;

        assert!(
            got_unregister.is_ok(),
            "client never sent Unregister after shutdown signal"
        );
        let _ = tx.send(ShutdownRelayEvent::UnregisterReceived);
    });

    (port, rx)
}

#[cfg(unix)]
async fn assert_graceful_shutdown(signal: i32) {
    let (relay_port, mut events) = spawn_shutdown_relay().await;
    let mut child = spawn_child(relay_port, 3000);

    // Wait until the client has registered with the relay.
    let registered = tokio::time::timeout(Duration::from_secs(15), events.recv())
        .await
        .expect("client never registered before signal")
        .expect("relay event channel closed early");
    assert!(
        matches!(registered, ShutdownRelayEvent::Registered),
        "expected Registered, got {registered:?}"
    );

    // Give the client a beat to spin up its session loop, then signal it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    // SAFETY: `child.id()` is a valid live PID we spawned ourselves.
    unsafe {
        libc::kill(child.id() as i32, signal);
    }

    // The relay must observe the graceful Unregister.
    let unregister = tokio::time::timeout(Duration::from_secs(15), events.recv())
        .await
        .expect("relay never received Unregister after signal")
        .expect("relay event channel closed early");
    assert!(
        matches!(unregister, ShutdownRelayEvent::UnregisterReceived),
        "expected UnregisterReceived, got {unregister:?}"
    );

    // The process must exit cleanly with code 0 (not 130 or a fatal error).
    let code = wait_for_exit(&mut child, Duration::from_secs(15)).await;
    assert_eq!(code, Some(0), "graceful shutdown must exit with code 0");
    let _ = child.wait();
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_full_proxy_flow_serve_200() {
    let local_port = spawn_mock_local(b"hello from localshare e2e").await;
    let (relay_port, rx) = spawn_mock_relay(false).await;

    let child = spawn_child(relay_port, local_port);

    let outcome = tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .expect("mock relay timed out: full flow did not complete")
        .expect("mock relay task failed");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let stdout = kill_and_drain(child);

    match outcome {
        RelayOutcome::Served { status, body } => {
            assert_eq!(status, 200, "client should report HTTP 200");
            assert_eq!(body, b"hello from localshare e2e", "body mismatch");
        }
        other => panic!("expected Served, got {:?}", other),
    }

    // The client should have streamed the request log as JSON on stdout.
    assert!(
        stdout.contains("\"event\":\"connected\""),
        "expected connected event in JSON"
    );
    assert!(
        stdout.contains("\"status\":200"),
        "expected request_handled with status 200"
    );
}

#[tokio::test]
async fn e2e_local_connection_refused_reports_502_with_hint() {
    // Reserve a port and release it so the local server is definitively down.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind temp");
    let closed_port = listener.local_addr().expect("temp addr").port();
    drop(listener);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (relay_port, rx) = spawn_mock_relay(true).await;

    let child = spawn_child(relay_port, closed_port);

    let outcome = tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .expect("mock relay timed out: refusal did not reach relay")
        .expect("mock relay task failed");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let stdout = kill_and_drain(child);

    match outcome {
        RelayOutcome::Refused { message } => {
            assert!(
                message.to_ascii_lowercase().contains("refused"),
                "expected connection refused message, got: {message}"
            );
        }
        other => panic!("expected Refused, got {:?}", other),
    }

    // The client synthesises a 502 and surfaces an actionable hint in JSON.
    assert!(
        stdout.contains("\"status\":502"),
        "expected request_handled with 502"
    );
    assert!(
        stdout.contains("Is your local server running on 127.0.0.1:"),
        "expected actionable hint in JSON"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn e2e_sigint_gracefully_unregisters_and_exits_zero() {
    assert_graceful_shutdown(libc::SIGINT).await;
}

#[cfg(unix)]
#[tokio::test]
async fn e2e_sigterm_gracefully_unregisters_and_exits_zero() {
    assert_graceful_shutdown(libc::SIGTERM).await;
}

// ── Failure recovery: fault then next request succeeds ────────────────────────

/// Shared assertion for the two failure-recovery scenarios: the first request
/// hits a faulty local server and the client synthesises a 502, then a second
/// request is served 200 — proving the forwarding loop neither spins nor dies.
async fn assert_fault_then_next_request_ok(behavior: FaultBehavior) {
    let target_port = spawn_faulty_then_healthy(behavior).await;
    let (relay_port, rx) = spawn_mock_relay_error_then_ok().await;

    let child = spawn_child(relay_port, target_port);

    let (err_code, status2) = tokio::time::timeout(Duration::from_secs(20), rx)
        .await
        .expect("mock relay timed out")
        .expect("mock relay task failed");

    assert_eq!(
        err_code,
        ResponseErrorCode::LocalIoError,
        "first request must yield a LocalIoError ResponseError"
    );
    assert_eq!(status2, 200, "second request must be served with HTTP 200");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let stdout = kill_and_drain(child);

    assert!(
        stdout.contains("\"status\":502"),
        "client must have logged a 502 for the failed request"
    );
    assert!(
        stdout.contains("\"status\":200"),
        "client must have logged a 200 for the recovered request"
    );
}

#[tokio::test]
async fn e2e_target_drop_mid_request_then_next_request_succeeds() {
    assert_fault_then_next_request_ok(FaultBehavior::DropAfterAccept).await;
}

#[tokio::test]
async fn e2e_malformed_response_then_next_request_succeeds() {
    assert_fault_then_next_request_ok(FaultBehavior::MalformedResponse).await;
}

// ── Sudden network loss: relay connection dropped mid-session ──────────────────

/// Spawn a mock relay that registers a client, proxies one request (200), then
/// abruptly closes the TCP connection with *no* WebSocket close frame — a brute
/// network loss, not a graceful relay shutdown. The client must treat that as a
/// transport error, enter the reconnect loop, register again, and serve request
/// two (200). Both status codes are reported over `oneshot`.
async fn spawn_dropping_relay() -> (u16, oneshot::Receiver<(u16, u16)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let port = listener.local_addr().expect("relay addr").port();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let deadline = Duration::from_secs(20);
        let mut status1: Option<u16> = None;

        // Connection 1: register, serve request 1, then drop the socket.
        {
            let (tcp, _) = listener.accept().await.expect("accept first connection");
            let mut ws = accept_async(tcp).await.expect("ws handshake 1");

            let first = ws
                .next()
                .await
                .expect("client should send Register")
                .expect("frame ok");
            let msg = Message::from_ws(&first).expect("first frame should parse");
            assert!(matches!(msg.payload, Payload::Register { .. }));
            ws.send(
                Message::new(Payload::Registered {
                    subdomain: "e2e-netloss".into(),
                    public_url: "https://e2e-netloss.localshare.dev".into(),
                    heartbeat_interval_ms: 60_000,
                })
                .to_binary(),
            )
            .await
            .expect("send Registered");

            ws.send(
                Message::new(Payload::RequestStart {
                    stream_id: 1,
                    method: "GET".into(),
                    path: "/one".into(),
                    headers: vec![],
                })
                .to_binary(),
            )
            .await
            .expect("send RequestStart 1");
            ws.send(Message::new(Payload::RequestEnd { stream_id: 1 }).to_binary())
                .await
                .expect("send RequestEnd 1");

            while status1.is_none() {
                let frame = tokio::time::timeout(deadline, ws.next())
                    .await
                    .expect("timed out waiting for first response")
                    .expect("websocket closed before first response")
                    .expect("websocket error");
                let Some(msg) = Message::from_ws(&frame) else {
                    continue;
                };
                match msg.payload {
                    Payload::ClientPing => {
                        ws.send(Message::new(Payload::RelayPong).to_binary())
                            .await
                            .expect("send RelayPong");
                    }
                    Payload::ResponseStart { status_code, .. } => status1 = Some(status_code),
                    _ => {}
                }
            }
            // ws (and its TCP socket) drop here: no close frame, no Unregister.
        }

        // Connection 2: the client must have reconnected and re-registered.
        let (tcp, _) = listener.accept().await.expect("accept reconnected client");
        let mut ws = accept_async(tcp).await.expect("ws handshake 2");

        let first = ws
            .next()
            .await
            .expect("client should re-register")
            .expect("frame ok");
        let msg = Message::from_ws(&first).expect("second frame should parse");
        assert!(matches!(msg.payload, Payload::Register { .. }));
        ws.send(
            Message::new(Payload::Registered {
                subdomain: "e2e-netloss".into(),
                public_url: "https://e2e-netloss.localshare.dev".into(),
                heartbeat_interval_ms: 60_000,
            })
            .to_binary(),
        )
        .await
        .expect("send Registered");

        ws.send(
            Message::new(Payload::RequestStart {
                stream_id: 2,
                method: "GET".into(),
                path: "/two".into(),
                headers: vec![],
            })
            .to_binary(),
        )
        .await
        .expect("send RequestStart 2");
        ws.send(Message::new(Payload::RequestEnd { stream_id: 2 }).to_binary())
            .await
            .expect("send RequestEnd 2");

        let mut status2: Option<u16> = None;
        while status2.is_none() {
            let frame = tokio::time::timeout(deadline, ws.next())
                .await
                .expect("timed out waiting for second response")
                .expect("websocket closed before second response")
                .expect("websocket error");
            let Some(msg) = Message::from_ws(&frame) else {
                continue;
            };
            match msg.payload {
                Payload::ClientPing => {
                    ws.send(Message::new(Payload::RelayPong).to_binary())
                        .await
                        .expect("send RelayPong");
                }
                Payload::ResponseStart { status_code, .. } => status2 = Some(status_code),
                _ => {}
            }
        }

        let _ = tx.send((status1.expect("status1"), status2.expect("status2")));
    });

    (port, rx)
}

/// The client must survive a mid-session network loss: after the relay drops
/// the connection without a close frame, the tunnel reconnects (with backoff),
/// re-registers, and keeps proxying. A healthy second request proves it.
#[tokio::test]
async fn e2e_client_reconnects_and_serves_after_relay_network_loss() {
    let target_port = spawn_mock_local_n(b"served after reconnect", 2).await;
    let (relay_port, rx) = spawn_dropping_relay().await;

    let child = spawn_child(relay_port, target_port);

    let (status1, status2) = match tokio::time::timeout(Duration::from_secs(25), rx).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => panic!("mock relay task failed: {e:?}"),
        Err(_) => {
            let stdout = kill_and_drain(child);
            panic!("mock relay timed out (client did not reconnect?); child stdout:\n{stdout}");
        }
    };

    assert_eq!(status1, 200, "first request must be served before the drop");
    assert_eq!(
        status2, 200,
        "second request must be served after reconnect"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    let stdout = kill_and_drain(child);

    assert!(
        stdout.contains("\"status\":200"),
        "client must have logged the proxied request"
    );
}
