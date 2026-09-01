use thiserror::Error;

// Re-export the canonical serde-compatible RejectReason from the protocol layer.
// Having a single definition avoids conversion boilerplate and keeps the type
// visible through `crate::error::RejectReason` for callers that don't import
// the full tunnel module.
pub use crate::tunnel::protocol::RejectReason;

// ── Relay-level errors ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum RelayError {
    #[error("connection to relay {0} failed: {1}")]
    Connect(String, String),

    #[error("handshake with relay {0} timed out after {1:?}")]
    HandshakeTimeout(String, std::time::Duration),

    #[error("registration rejected by relay: {0:?}")]
    RegistrationRejected(RejectReason),

    #[error("unexpected relay WebSocket close: {0}")]
    UnexpectedClose(String),

    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u8, got: u8 },

    #[error("invalid relay URL '{0}': {1}")]
    InvalidUrl(String, String),

    #[error("relay WebSocket I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("relay message serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ── Local forwarding errors ───────────────────────────────────────────────────

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum LocalForwardError {
    #[error("connection refused to local target {0}")]
    TargetConnectionRefused(String),

    #[error("local TCP I/O error while forwarding to {0}: {1}")]
    TcpIo(String, #[source] std::io::Error),

    #[error("failed to parse local HTTP response from {0}: {1}")]
    ResponseParse(String, String),

    #[error("local target operation timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("unexpected local stream end on {0}")]
    StreamEnded(String),
}

#[allow(dead_code)]
impl LocalForwardError {
    // ── Constructor helpers ───────────────────────────────────────────────────

    pub fn target_connection_refused(host: impl Into<String>, port: u16) -> Self {
        Self::TargetConnectionRefused(format!("{}:{}", host.into(), port))
    }

    pub fn tcp_io(host: impl Into<String>, port: u16, source: std::io::Error) -> Self {
        Self::TcpIo(format!("{}:{}", host.into(), port), source)
    }

    pub fn response_parse(host: impl Into<String>, port: u16, reason: impl Into<String>) -> Self {
        Self::ResponseParse(format!("{}:{}", host.into(), port), reason.into())
    }

    pub fn timeout(duration: std::time::Duration) -> Self {
        Self::Timeout(duration)
    }

    pub fn stream_ended(host: impl Into<String>, port: u16) -> Self {
        Self::StreamEnded(format!("{}:{}", host.into(), port))
    }

    // ── User-facing actionable hints ──────────────────────────────────────────

    /// Returns a short, human-readable suggestion to help the user fix the
    /// underlying problem, if one can be inferred from the error variant.
    pub fn actionable_hint(&self) -> Option<&str> {
        match self {
            Self::TargetConnectionRefused(addr) => {
                // We store "host:port" in the address field.
                let _ = addr;
                Some("Is your local server running? Check the port number and try again.")
            }
            Self::Timeout(_) => {
                Some("The local server took too long to respond. Consider increasing the timeout or checking for blocking operations.")
            }
            Self::StreamEnded(_) => {
                Some("The local server closed the connection unexpectedly. Check its logs for errors.")
            }
            _ => None,
        }
    }
}

// ── Protocol-level errors ─────────────────────────────────────────────────────

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ProtocolError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("unexpected message type '{0}', expected '{1}'")]
    UnexpectedMessageType(String, String),

    #[error("unknown stream id {0}")]
    UnknownStreamId(u64),

    #[error("invalid message frame: {0}")]
    InvalidFrame(String),

    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u8, got: u8 },
}

// ── CLI-level errors ──────────────────────────────────────────────────────────

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum CliError {
    #[error("invalid target: {0}")]
    InvalidTarget(String),

    #[error("non-interactive mode conflict: {0}")]
    NonInteractiveConflict(String),
}
