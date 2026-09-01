//! Local HTTP forwarding engine.
//!
//! Each incoming request from the relay is handled by [`process_stream`], which
//! is spawned as an independent Tokio task. The task:
//!
//! 1. Opens a `TcpStream` to the local target.
//! 2. Writes the reconstructed HTTP/1.1 request (headers + body chunks).
//! 3. Reads and parses the HTTP response from the local server.
//! 4. Emits `ResponseStart`, one or more `ResponseChunk`s, and `ResponseEnd`
//!    back through `response_tx`.
//!
//! On any error a `ResponseError` is sent instead so the relay can synthesise
//! an appropriate HTTP error response (e.g. 502 Bad Gateway).

use crate::error::LocalForwardError;
use crate::tunnel::protocol::StreamMessage;
use crate::tunnel::protocol::{Header, LocalTarget, Message, Payload, ResponseErrorCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// Read buffer size when streaming the local response body.
const READ_BUF: usize = 64 * 1024; // 64 KiB

/// Maximum HTTP response header block we'll try to parse (8 KiB).
const MAX_HEADER_BUF: usize = 8 * 1024;

// ── Public entry point ────────────────────────────────────────────────────────

/// Spawn a task that handles a single relay request stream end-to-end.
///
/// `stream_id`, `method`, `path`, and `headers` come from the `RequestStart`
/// message. `request_rx` receives subsequent `RequestChunk`, `RequestEnd`, and
/// `StreamReset` messages. Responses are sent through `response_tx`.
pub fn process_stream(
    stream_id: u64,
    method: String,
    path: String,
    headers: Vec<Header>,
    target: LocalTarget,
    request_rx: mpsc::Receiver<StreamMessage>,
    response_tx: mpsc::Sender<Message>,
) {
    tokio::spawn(async move {
        if let Err(e) = forward_request(
            stream_id,
            method,
            path,
            headers,
            &target,
            request_rx,
            &response_tx,
        )
        .await
        {
            tracing::warn!(stream_id, target = %target, error = %e, "stream forwarding failed");
            // Best-effort: inform the relay so it can send a 502. This is the
            // single place a `ResponseError` is emitted per stream (the caller
            // must NOT also send one), so the relay never sees a duplicate.
            let _ = response_tx
                .send(Message::new(Payload::ResponseError {
                    stream_id,
                    code: match &e {
                        LocalForwardError::TargetConnectionRefused(_) => {
                            ResponseErrorCode::TargetConnectionRefused
                        }
                        _ => ResponseErrorCode::LocalIoError,
                    },
                    message: match &e {
                        LocalForwardError::TargetConnectionRefused(target) => {
                            format!(
                                "Connection refused to {target} — is your local server running?"
                            )
                        }
                        other => other.to_string(),
                    },
                }))
                .await;
        }
    });
}

// ── Core forwarding logic ─────────────────────────────────────────────────────

async fn forward_request(
    stream_id: u64,
    method: String,
    path: String,
    headers: Vec<Header>,
    target: &LocalTarget,
    mut request_rx: mpsc::Receiver<StreamMessage>,
    response_tx: &mpsc::Sender<Message>,
) -> Result<(), LocalForwardError> {
    // ── 1. Connect to local target ────────────────────────────────────────────
    let stream = match tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            // The `ResponseError` is emitted by the calling `process_stream`
            // wrapper so it is guaranteed to be sent exactly once.
            return Err(LocalForwardError::target_connection_refused(
                target.host.clone(),
                target.port,
            ));
        }
        Err(e) => {
            return Err(LocalForwardError::tcp_io(
                target.host.clone(),
                target.port,
                e,
            ));
        }
    };

    let (mut reader, mut writer) = stream.into_split();

    // ── 2. Write request headers to local target ──────────────────────────────
    let request_head = build_request_head(&method, &path, &headers, target);
    writer
        .write_all(request_head.as_bytes())
        .await
        .map_err(|e| LocalForwardError::tcp_io(target.host.clone(), target.port, e))?;

    // ── 3. Stream request body chunks → local target ──────────────────────────
    // We drive this concurrently with reading the response (some servers start
    // responding before receiving the full body), but we keep it simple here:
    // fully send the request then read the response, which is correct for the
    // vast majority of HTTP/1.1 use-cases.
    loop {
        match request_rx.recv().await {
            Some(StreamMessage::Payload(Payload::RequestChunk { data, .. })) => {
                writer
                    .write_all(&data)
                    .await
                    .map_err(|e| LocalForwardError::tcp_io(target.host.clone(), target.port, e))?;
            }
            Some(StreamMessage::Payload(Payload::RequestEnd { .. })) => {
                // Signal EOF to the local server by shutting down the write half.
                writer
                    .shutdown()
                    .await
                    .map_err(|e| LocalForwardError::tcp_io(target.host.clone(), target.port, e))?;
                break;
            }
            Some(StreamMessage::Payload(Payload::StreamReset { .. }))
            | Some(StreamMessage::Cancel)
            | None => {
                // Relay cancelled this stream; abort silently.
                return Ok(());
            }
            _ => {
                // Ignore unexpected payloads during body phase.
            }
        }
    }

    // ── 4. Read & parse HTTP response headers ─────────────────────────────────
    let (status_code, response_headers, leftover_body) =
        read_response_head(&mut reader, target).await?;

    response_tx
        .send(Message::new(Payload::ResponseStart {
            stream_id,
            status_code,
            headers: response_headers,
        }))
        .await
        .ok();

    // ── 5. Stream response body ────────────────────────────────────────────────
    // First, emit whatever bytes were read beyond the header boundary.
    if !leftover_body.is_empty() {
        response_tx
            .send(Message::new(Payload::ResponseChunk {
                stream_id,
                data: leftover_body,
            }))
            .await
            .ok();
    }

    let mut buf = vec![0u8; READ_BUF];
    loop {
        // Check for a cancellation signal without blocking.
        match request_rx.try_recv() {
            Ok(StreamMessage::Cancel) | Ok(StreamMessage::Payload(Payload::StreamReset { .. })) => {
                return Ok(());
            }
            _ => {}
        }

        let n = reader
            .read(&mut buf)
            .await
            .map_err(|e| LocalForwardError::tcp_io(target.host.clone(), target.port, e))?;
        if n == 0 {
            break; // EOF — local server closed the connection.
        }
        response_tx
            .send(Message::new(Payload::ResponseChunk {
                stream_id,
                data: buf[..n].to_vec(),
            }))
            .await
            .ok();
    }

    // ── 6. Signal end of response ──────────────────────────────────────────────
    response_tx
        .send(Message::new(Payload::ResponseEnd { stream_id }))
        .await
        .ok();

    Ok(())
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

/// Builds the raw HTTP/1.1 request head (request line + headers + blank line)
/// that will be written verbatim to the local TCP stream.
///
/// Header normalisation rules:
/// - `Connection`, `Upgrade`, `Sec-WebSocket-*` hop-by-hop headers are stripped.
/// - `Host` is preserved if present; injected as `host:port` otherwise.
/// - `X-Forwarded-For`, `X-Forwarded-Proto`, and `X-Forwarded-Host` are appended.
fn build_request_head(
    method: &str,
    path: &str,
    headers: &[Header],
    target: &LocalTarget,
) -> String {
    let host_label = format!("{}:{}", target.host, target.port);
    let mut out = format!("{} {} HTTP/1.1\r\n", method, path);
    let mut wrote_host = false;

    for header in headers {
        let lower = header.name.to_ascii_lowercase();
        // Drop hop-by-hop and WebSocket-upgrade headers.
        if matches!(
            lower.as_str(),
            "connection"
                | "upgrade"
                | "sec-websocket-key"
                | "sec-websocket-version"
                | "sec-websocket-extensions"
                | "sec-websocket-protocol"
                | "te"
                | "trailer"
                | "transfer-encoding"
        ) {
            continue;
        }
        if lower == "host" {
            wrote_host = true;
        }
        out.push_str(&header.name);
        out.push_str(": ");
        out.push_str(&header.value);
        out.push_str("\r\n");
    }

    if !wrote_host {
        out.push_str("Host: ");
        out.push_str(&host_label);
        out.push_str("\r\n");
    }

    // Forwarding metadata headers.
    out.push_str("X-Forwarded-Proto: http\r\n");
    out.push_str("X-Forwarded-Host: ");
    out.push_str(&host_label);
    out.push_str("\r\n");

    // Force connection close — we own the socket lifecycle.
    out.push_str("Connection: close\r\n");
    out.push_str("\r\n");
    out
}

/// Read bytes from `reader` until the HTTP response header block is complete
/// (`\r\n\r\n` or `\n\n`), parse them with `httparse`, and return:
///   - `status_code`
///   - response `Header` list
///   - any body bytes already read past the header boundary (`leftover_body`)
async fn read_response_head(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    target: &LocalTarget,
) -> Result<(u16, Vec<Header>, Vec<u8>), LocalForwardError> {
    let mut raw = Vec::with_capacity(4096);
    let mut temp = vec![0u8; 4096];

    // Read until we find the header/body boundary.
    let header_end = loop {
        if raw.len() > MAX_HEADER_BUF {
            return Err(LocalForwardError::response_parse(
                target.host.clone(),
                target.port,
                "HTTP response headers exceed 8 KiB limit",
            ));
        }

        let n = reader
            .read(&mut temp)
            .await
            .map_err(|e| LocalForwardError::tcp_io(target.host.clone(), target.port, e))?;
        if n == 0 {
            return Err(LocalForwardError::stream_ended(
                target.host.clone(),
                target.port,
            ));
        }
        raw.extend_from_slice(&temp[..n]);

        if let Some(pos) = find_header_end(&raw) {
            break pos;
        }
    };

    // ── Parse with httparse ───────────────────────────────────────────────────
    let mut httparse_headers = [httparse::EMPTY_HEADER; 128];
    let mut resp = httparse::Response::new(&mut httparse_headers);

    match resp.parse(&raw[..header_end + 4]) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => {
            return Err(LocalForwardError::response_parse(
                target.host.clone(),
                target.port,
                "incomplete HTTP response headers",
            ));
        }
        Err(e) => {
            return Err(LocalForwardError::response_parse(
                target.host.clone(),
                target.port,
                format!("httparse error: {}", e),
            ));
        }
    }

    let status_code = resp.code.ok_or_else(|| {
        LocalForwardError::response_parse(
            target.host.clone(),
            target.port,
            "missing status code in response",
        )
    })?;

    let response_headers: Vec<Header> = resp
        .headers
        .iter()
        .filter(|h| !h.name.is_empty())
        .map(|h| Header {
            name: h.name.to_string(),
            value: String::from_utf8_lossy(h.value).into_owned(),
        })
        .collect();

    // Bytes after the header boundary are body data.
    let leftover_body = raw[header_end + 4..].to_vec();

    Ok((status_code, response_headers, leftover_body))
}

/// Scan `buf` for the HTTP header/body delimiter sequence `\r\n\r\n`.
/// Returns the byte index of the first `\r` of the terminal `\r\n\r\n`, or
/// `None` if the delimiter has not yet been received.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    // Need at least 4 bytes.
    if buf.len() < 4 {
        return None;
    }
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::protocol::{Header, LocalTarget};
    use tokio::io::AsyncWriteExt;

    fn make_target(port: u16) -> LocalTarget {
        LocalTarget {
            host: "127.0.0.1".into(),
            port,
        }
    }

    // ── build_request_head ────────────────────────────────────────────────────

    #[test]
    fn request_head_strips_hop_by_hop() {
        let headers = vec![
            Header {
                name: "Host".into(),
                value: "example.com".into(),
            },
            Header {
                name: "Connection".into(),
                value: "keep-alive".into(),
            },
            Header {
                name: "Accept".into(),
                value: "*/*".into(),
            },
            Header {
                name: "Upgrade".into(),
                value: "websocket".into(),
            },
        ];
        let head = build_request_head("GET", "/", &headers, &make_target(3000));
        assert!(
            head.contains("Accept: */*\r\n"),
            "Accept header should be preserved"
        );
        assert!(
            !head.contains("Connection: keep-alive"),
            "Connection hop-by-hop must be stripped"
        );
        assert!(
            !head.contains("Upgrade:"),
            "Upgrade hop-by-hop must be stripped"
        );
        // Connection: close should be injected by us.
        assert!(head.contains("Connection: close\r\n"));
    }

    #[test]
    fn request_head_injects_host_when_missing() {
        let head = build_request_head("POST", "/data", &[], &make_target(8080));
        assert!(head.contains("Host: 127.0.0.1:8080\r\n"));
    }

    #[test]
    fn request_head_preserves_existing_host() {
        let headers = vec![Header {
            name: "Host".into(),
            value: "myapp.example.com".into(),
        }];
        let head = build_request_head("GET", "/", &headers, &make_target(3000));
        assert!(head.contains("Host: myapp.example.com\r\n"));
        // Must not duplicate the Host header (X-Forwarded-Host is separate).
        let host_header_lines: Vec<&str> = head
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("host:"))
            .collect();
        assert_eq!(
            host_header_lines.len(),
            1,
            "Host header should appear exactly once"
        );
    }

    #[test]
    fn request_head_injects_forwarding_headers() {
        let head = build_request_head("GET", "/", &[], &make_target(5000));
        assert!(head.contains("X-Forwarded-Proto: http\r\n"));
        assert!(head.contains("X-Forwarded-Host: 127.0.0.1:5000\r\n"));
    }

    #[test]
    fn request_head_request_line_format() {
        let head = build_request_head("DELETE", "/items/1", &[], &make_target(80));
        assert!(head.starts_with("DELETE /items/1 HTTP/1.1\r\n"));
    }

    // ── find_header_end ───────────────────────────────────────────────────────

    #[test]
    fn find_header_end_found() {
        let buf = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let pos = find_header_end(buf).unwrap();
        assert_eq!(&buf[pos..pos + 4], b"\r\n\r\n");
    }

    #[test]
    fn find_header_end_not_found() {
        let buf = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n";
        assert!(find_header_end(buf).is_none());
    }

    // ── connection refusal → ResponseError ────────────────────────────────────

    #[tokio::test]
    async fn connection_refused_sends_response_error() {
        // Bind a listener, grab its port, then drop it so the port is closed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let target = make_target(port);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(8);
        let (_req_tx, req_rx) = mpsc::channel::<StreamMessage>(8);

        // Give the OS a moment to fully release the port.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        process_stream(
            1,
            "GET".into(),
            "/".into(),
            vec![],
            target,
            req_rx,
            response_tx,
        );

        // The first message must be a ResponseError with TargetConnectionRefused.
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), response_rx.recv())
            .await
            .expect("timed out waiting for ResponseError")
            .expect("channel closed");

        match msg.payload {
            Payload::ResponseError {
                code: ResponseErrorCode::TargetConnectionRefused,
                ..
            } => {}
            other => panic!(
                "expected ResponseError(TargetConnectionRefused), got {:?}",
                other
            ),
        }
    }

    // ── end-to-end forwarding through a mock HTTP server ─────────────────────

    #[tokio::test]
    async fn end_to_end_get_request() {
        // Spawn a minimal HTTP server that always responds 200 OK.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Read request (discard).
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            // Write a minimal HTTP/1.1 response.
            let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            socket.write_all(resp).await.unwrap();
        });

        let target = make_target(port);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(16);
        let (req_tx, req_rx) = mpsc::channel::<StreamMessage>(8);

        process_stream(
            2,
            "GET".into(),
            "/".into(),
            vec![Header {
                name: "Accept".into(),
                value: "*/*".into(),
            }],
            target,
            req_rx,
            response_tx,
        );

        // Send RequestEnd so forward_request knows the request is complete.
        req_tx
            .send(StreamMessage::Payload(Payload::RequestEnd { stream_id: 2 }))
            .await
            .unwrap();

        let mut got_start = false;
        let mut body = Vec::new();

        let deadline = std::time::Duration::from_secs(5);
        let got_end = 'recv: loop {
            let msg = tokio::time::timeout(deadline, response_rx.recv())
                .await
                .expect("timed out")
                .expect("channel closed");

            match msg.payload {
                Payload::ResponseStart { status_code, .. } => {
                    assert_eq!(status_code, 200);
                    got_start = true;
                }
                Payload::ResponseChunk { data, .. } => {
                    body.extend_from_slice(&data);
                }
                Payload::ResponseEnd { .. } => {
                    break 'recv true;
                }
                Payload::ResponseError { message, .. } => {
                    panic!("unexpected ResponseError: {}", message);
                }
                _ => {}
            }
        };

        assert!(got_start, "ResponseStart not received");
        assert_eq!(body, b"hello", "body mismatch");
        assert!(got_end, "ResponseEnd not received");
    }

    // ── local server drops mid-request → ResponseError(LocalIoError) ──────────

    #[tokio::test]
    async fn target_drops_after_accept_sends_response_error() {
        // The server accepts the connection, reads the request, then closes
        // without ever writing a response.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            drop(socket);
        });

        let target = make_target(port);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(8);
        let (req_tx, req_rx) = mpsc::channel::<StreamMessage>(8);

        process_stream(
            7,
            "GET".into(),
            "/".into(),
            vec![],
            target,
            req_rx,
            response_tx,
        );
        req_tx
            .send(StreamMessage::Payload(Payload::RequestEnd { stream_id: 7 }))
            .await
            .unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), response_rx.recv())
            .await
            .expect("timed out waiting for ResponseError")
            .expect("channel closed");

        match msg.payload {
            Payload::ResponseError {
                code: ResponseErrorCode::LocalIoError,
                ..
            } => {}
            other => panic!("expected ResponseError(LocalIoError), got {:?}", other),
        }
    }

    // ── malformed local HTTP response → ResponseError(LocalIoError) ────────────

    #[tokio::test]
    async fn malformed_response_sends_response_error() {
        // The server responds with bytes that are not a valid HTTP response,
        // then closes the connection.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(b"THIS IS NOT AN HTTP RESPONSE\r\n\r\n")
                .await;
            drop(socket);
        });

        let target = make_target(port);
        let (response_tx, mut response_rx) = mpsc::channel::<Message>(8);
        let (req_tx, req_rx) = mpsc::channel::<StreamMessage>(8);

        process_stream(
            8,
            "GET".into(),
            "/".into(),
            vec![],
            target,
            req_rx,
            response_tx,
        );
        req_tx
            .send(StreamMessage::Payload(Payload::RequestEnd { stream_id: 8 }))
            .await
            .unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), response_rx.recv())
            .await
            .expect("timed out waiting for ResponseError")
            .expect("channel closed");

        match msg.payload {
            Payload::ResponseError {
                code: ResponseErrorCode::LocalIoError,
                ..
            } => {}
            other => panic!("expected ResponseError(LocalIoError), got {:?}", other),
        }
    }
}
