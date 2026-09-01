use crate::error::{ProtocolError, RelayError};
use serde::{Deserialize, Serialize};
use url::Url;

pub const PROTOCOL_VERSION: u8 = 1;

// ── Top-level wire message ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub version: u8,
    pub payload: Payload,
}

impl Message {
    pub fn new(payload: Payload) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            payload,
        }
    }

    /// Serialize this message into a WebSocket binary frame.
    pub fn into_ws_message(self) -> tungstenite::Message {
        let v = serde_json::to_vec(&self).expect("Message serialization is infallible");
        tungstenite::Message::Binary(v)
    }

    /// Deserialize a `Message` from a text or binary WebSocket frame.
    /// Returns a `ProtocolError` for control frames, raw frames, or
    /// messages with a mismatched protocol version.
    pub fn from_ws_message(msg: &tungstenite::Message) -> Result<Self, ProtocolError> {
        let data: &[u8] = match msg {
            // In tungstenite 0.21 Text holds a String; coerce to bytes.
            tungstenite::Message::Text(s) => s.as_bytes(),
            tungstenite::Message::Binary(b) => b.as_ref(),
            tungstenite::Message::Ping(_)
            | tungstenite::Message::Pong(_)
            | tungstenite::Message::Close(_) => {
                return Err(ProtocolError::InvalidFrame(
                    "expected text or binary frame, got control frame".into(),
                ))
            }
            tungstenite::Message::Frame(_) => {
                return Err(ProtocolError::InvalidFrame(
                    "raw frame not supported".into(),
                ))
            }
        };

        let message: Message = serde_json::from_slice(data)?;
        if message.version != PROTOCOL_VERSION {
            return Err(ProtocolError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                got: message.version,
            });
        }
        Ok(message)
    }
}

// ── Payload variants ──────────────────────────────────────────────────────────

/// All messages exchanged between client and relay.
///
/// Direction guide:
///   Client → Relay: `Register`, `ClientPing`, `ResponseStart`, `ResponseChunk`,
///                   `ResponseEnd`, `ResponseError`, `Unregister`
///   Relay → Client: `Registered`, `RegisterRejected`, `RelayPong`,
///                   `RequestStart`, `RequestChunk`, `RequestEnd`, `StreamReset`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Payload {
    // ── Client → Relay ────────────────────────────────────────────────────────
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

    // ── Relay → Client ────────────────────────────────────────────────────────
    Registered {
        subdomain: String,
        public_url: String,
        heartbeat_interval_ms: u64,
    },
    RegisterRejected {
        reason: RejectReason,
    },
    RelayPong,
    RequestStart {
        stream_id: u64,
        method: String,
        path: String,
        headers: Vec<Header>,
    },
    RequestChunk {
        stream_id: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    RequestEnd {
        stream_id: u64,
    },
    StreamReset {
        stream_id: u64,
    },
}

// ── Supporting types ──────────────────────────────────────────────────────────

/// A single HTTP header preserving both name and value, including duplicates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// Application-level error codes sent from the client to the relay so the
/// relay can synthesise an appropriate HTTP error response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseErrorCode {
    TargetConnectionRefused,
    TargetTimeout,
    LocalIoError,
}

/// Reason why the relay rejected a registration request.
/// This is the canonical definition; `crate::error::RejectReason` re-exports it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    SubdomainTaken,
    ServerFull,
    UnsupportedClient,
    Other,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubdomainTaken => write!(f, "subdomain already taken"),
            Self::ServerFull => write!(f, "relay server is full"),
            Self::UnsupportedClient => write!(f, "client version not supported"),
            Self::Other => write!(f, "registration denied by relay"),
        }
    }
}

// ── Local target ──────────────────────────────────────────────────────────────

pub use crate::cli::LocalTarget;

// ── Relay endpoint ────────────────────────────────────────────────────────────

/// A parsed and validated relay server address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl RelayEndpoint {
    /// Produces the full WebSocket URL string suitable for `connect_async`.
    pub fn ws_url(&self) -> String {
        format!("{}://{}:{}{}", self.scheme, self.host, self.port, self.path)
    }
}

impl std::fmt::Display for RelayEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}:{}", self.scheme, self.host, self.port)
    }
}

/// Parse a relay address string into a validated `RelayEndpoint`.
///
/// Accepts:
/// - Bare hostname:        `relay.example.com`
/// - Host + port:          `relay.example.com:4433`
/// - Full WS URL:          `ws://relay.example.com/path`
/// - Full WSS URL:         `wss://relay.example.com:4433`
///
/// When no scheme is present the heuristic defaults to `ws://`; callers that
/// want TLS should prefix `wss://` explicitly or use port 443 logic above.
pub fn parse_relay(input: &str) -> Result<RelayEndpoint, RelayError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(RelayError::InvalidUrl(
            input.to_string(),
            "relay address cannot be empty".into(),
        ));
    }

    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("ws://{}", input)
    };

    let parsed = Url::parse(&with_scheme)
        .map_err(|e| RelayError::InvalidUrl(input.to_string(), e.to_string()))?;

    if !matches!(parsed.scheme(), "ws" | "wss") {
        return Err(RelayError::InvalidUrl(
            input.to_string(),
            format!(
                "unsupported scheme '{}', use 'ws' or 'wss'",
                parsed.scheme()
            ),
        ));
    }

    let scheme = parsed.scheme().to_string();
    let host = parsed
        .host_str()
        .ok_or_else(|| RelayError::InvalidUrl(input.to_string(), "missing host".into()))?
        .to_string();

    let default_port = if scheme == "wss" { 443 } else { 80 };
    let port = parsed.port().unwrap_or(default_port);

    let raw_path = parsed.path();
    // Normalise to "/" when path is empty or bare slash.
    let mut path = if raw_path.is_empty() {
        "/".to_string()
    } else {
        raw_path.to_string()
    };
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if let Some(q) = parsed.query().filter(|q| !q.is_empty()) {
        path = format!("{}?{}", path, q);
    }

    Ok(RelayEndpoint {
        scheme,
        host,
        port,
        path,
    })
}

// ── Internal stream multiplexing ──────────────────────────────────────────────

/// Messages routed from the relay dispatcher to a per-stream forwarding task.
#[derive(Debug, Clone)]
pub enum StreamMessage {
    /// A relay payload that belongs to this stream.
    Payload(Payload),
    /// Instructs the stream worker to abort immediately (e.g. `StreamReset`).
    Cancel,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: round-trip a Payload through Message serialization.
    fn roundtrip(payload: Payload) -> Payload {
        let msg = Message::new(payload);
        let ws_msg = msg.into_ws_message();
        Message::from_ws_message(&ws_msg).unwrap().payload
    }

    #[test]
    fn roundtrip_register() {
        let p = Payload::Register {
            requested_subdomain: Some("my-app".into()),
            client_name: "localshare".into(),
            client_version: "0.1.0".into(),
            heartbeat_interval_ms: 15_000,
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_registered() {
        let p = Payload::Registered {
            subdomain: "my-app".into(),
            public_url: "https://my-app.relay.example.com".into(),
            heartbeat_interval_ms: 15_000,
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_register_rejected() {
        let p = Payload::RegisterRejected {
            reason: RejectReason::SubdomainTaken,
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_client_ping_relay_pong() {
        assert_eq!(roundtrip(Payload::ClientPing), Payload::ClientPing);
        assert_eq!(roundtrip(Payload::RelayPong), Payload::RelayPong);
    }

    #[test]
    fn roundtrip_request_start() {
        let p = Payload::RequestStart {
            stream_id: 42,
            method: "GET".into(),
            path: "/api/users?page=1".into(),
            headers: vec![Header {
                name: "Accept".into(),
                value: "application/json".into(),
            }],
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_request_chunk() {
        let p = Payload::RequestChunk {
            stream_id: 7,
            data: b"hello world".to_vec(),
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_request_end() {
        let p = Payload::RequestEnd { stream_id: 7 };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_response_start() {
        let p = Payload::ResponseStart {
            stream_id: 1,
            status_code: 200,
            headers: vec![
                Header {
                    name: "Content-Type".into(),
                    value: "text/plain".into(),
                },
                Header {
                    name: "Content-Length".into(),
                    value: "5".into(),
                },
            ],
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_response_chunk() {
        let p = Payload::ResponseChunk {
            stream_id: 1,
            data: vec![0u8, 1, 2, 255],
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_response_end() {
        assert_eq!(
            roundtrip(Payload::ResponseEnd { stream_id: 1 }),
            Payload::ResponseEnd { stream_id: 1 }
        );
    }

    #[test]
    fn roundtrip_response_error() {
        let p = Payload::ResponseError {
            stream_id: 3,
            code: ResponseErrorCode::TargetConnectionRefused,
            message: "refused".into(),
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_stream_reset() {
        let p = Payload::StreamReset { stream_id: 99 };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_unregister() {
        let p = Payload::Unregister {
            reason: Some("graceful shutdown".into()),
        };
        assert_eq!(roundtrip(p.clone()), p);
    }

    #[test]
    fn roundtrip_binary_payload_preserves_bytes() {
        // Ensure zero bytes and high bytes survive the serde_bytes encoding.
        let data: Vec<u8> = (0u8..=255).collect();
        let p = Payload::ResponseChunk {
            stream_id: 0,
            data: data.clone(),
        };
        match roundtrip(p) {
            Payload::ResponseChunk { data: got, .. } => assert_eq!(got, data),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // ── parse_relay URL normalization ─────────────────────────────────────────

    #[test]
    fn parse_bare_hostname() {
        let ep = parse_relay("relay.example.com").unwrap();
        assert_eq!(ep.scheme, "ws");
        assert_eq!(ep.host, "relay.example.com");
        assert_eq!(ep.port, 80);
        assert_eq!(ep.path, "/");
    }

    #[test]
    fn parse_bare_hostname_with_port() {
        let ep = parse_relay("relay.example.com:4433").unwrap();
        assert_eq!(ep.scheme, "ws");
        assert_eq!(ep.port, 4433);
    }

    #[test]
    fn parse_ws_url() {
        let ep = parse_relay("ws://relay.example.com/ws/v1").unwrap();
        assert_eq!(ep.scheme, "ws");
        assert_eq!(ep.path, "/ws/v1");
    }

    #[test]
    fn parse_wss_url_default_port() {
        let ep = parse_relay("wss://secure.relay.io").unwrap();
        assert_eq!(ep.scheme, "wss");
        assert_eq!(ep.port, 443);
    }

    #[test]
    fn parse_wss_url_custom_port() {
        let ep = parse_relay("wss://secure.relay.io:8443/v2").unwrap();
        assert_eq!(ep.port, 8443);
        assert_eq!(ep.path, "/v2");
    }

    #[test]
    fn parse_with_query_string() {
        let ep = parse_relay("ws://relay.example.com/path?token=abc").unwrap();
        assert_eq!(ep.path, "/path?token=abc");
    }

    #[test]
    fn parse_localhost_with_port() {
        let ep = parse_relay("localhost:9000").unwrap();
        assert_eq!(ep.host, "localhost");
        assert_eq!(ep.port, 9000);
    }

    #[test]
    fn parse_empty_input_errors() {
        assert!(parse_relay("").is_err());
    }

    #[test]
    fn parse_unsupported_scheme_errors() {
        assert!(parse_relay("http://example.com").is_err());
    }

    #[test]
    fn relay_endpoint_ws_url() {
        let ep = RelayEndpoint {
            scheme: "wss".into(),
            host: "relay.example.com".into(),
            port: 443,
            path: "/ws".into(),
        };
        assert_eq!(ep.ws_url(), "wss://relay.example.com:443/ws");
    }

    #[test]
    fn version_mismatch_returns_error() {
        let msg = Message {
            version: 99,
            payload: Payload::ClientPing,
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let ws_msg = tungstenite::Message::Binary(json);
        match Message::from_ws_message(&ws_msg) {
            Err(ProtocolError::VersionMismatch { expected, got }) => {
                assert_eq!(expected, PROTOCOL_VERSION);
                assert_eq!(got, 99);
            }
            other => panic!("expected VersionMismatch, got {:?}", other),
        }
    }
}
