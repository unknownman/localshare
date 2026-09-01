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
use tokio::sync::oneshot;
use tokio_tungstenite::accept_async;

// ── Minimal on-the-wire protocol types ─────────────────────────────────────────
// Duplicated here intentionally: this test validates interop with the real
// binary over the wire, so it must not import the crate's internals.

#[derive(Debug, serde::Serialize, serde::Deserialize)]
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
        let deadline = Duration::from_secs(10);
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
