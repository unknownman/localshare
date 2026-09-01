//! Tunnel engine: relay protocol, connection handling, and local forwarding.
//!
//! The architecture is layered:
//!
//! - [`protocol`] — the wire messages exchanged with the relay
//!   (versioned JSON over WebSocket), plus parsing/validation helpers such as
//!   [`parse_relay`](protocol::parse_relay).
//! - [`client`] — the connection lifecycle: opening the WebSocket, the
//!   registration handshake, the heartbeat, an automatic reconnect loop with
//!   exponential backoff, and graceful shutdown on `SIGINT`/`SIGTERM`.
//! - [`forward`] — the local HTTP forwarding engine. Each relay request is
//!   proxied to the local target and streamed back; any failure is reported to
//!   the relay as a `ResponseError` so it can synthesise a 502.
//!
//! `client::run_tunnel` is the public entry point: it spawns the tunnel and
//! returns a `broadcast` channel of [`TunnelEvent`s](client::TunnelEvent) that
//! the UI layer consumes. The relay and client speak a JSON message protocol
//! tagged with a version number for forward compatibility.

pub mod client;
pub mod forward;
pub mod protocol;
