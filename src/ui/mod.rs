//! Terminal UI: rendering tunnel progress and errors for humans and scripts.
//!
//! [`run_ui`] consumes [`TunnelEvent`]s from the tunnel engine and renders them
//! in one of four modes selected by the CLI flags:
//!
//! - **interactive** (default) — a welcoming banner, QR code, and a live
//!   request log for a human at a terminal.
//! - **json** — machine-readable events, one JSON object per line.
//! - **quiet** — only the public URL and fatal errors.
//! - **log** — a plain stderr event stream that stays readable when verbose
//!   `tracing` output is enabled (`-vv` and above).
//!
//! The UI never enters raw mode, hides the cursor, or switches screens, so
//! there is no terminal state to restore on exit. Fatal tunnel failures are
//! rendered as an [`error_view::ErrorView`] with an actionable hint (see
//! [`format_disconnect`]) and surfaced to `main` as a `FatalTunnelError` so the
//! process can exit non-zero.

use crate::cli::Cli;
use crate::tunnel::client::{TunnelEvent, TunnelSession};
use std::io::{self, IsTerminal, Write};
use tokio::sync::broadcast;

pub mod banner;
pub mod error_view;
pub mod log;
pub mod qr;

/// Marker error returned by [`run_ui`] when the tunnel ended due to a fatal
/// error that has *already* been rendered to the user (via [`error_view`]).
/// `main` only needs to translate it into a non-zero exit code.
#[derive(Debug)]
pub struct FatalTunnelError;

impl std::fmt::Display for FatalTunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tunnel ended with a fatal error")
    }
}

impl std::error::Error for FatalTunnelError {}

pub async fn run_ui(
    config: &Cli,
    mut events: broadcast::Receiver<TunnelEvent>,
) -> anyhow::Result<()> {
    let supports_color = io::stdout().is_terminal();
    if std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }

    if config.json {
        return run_json_mode(&mut events).await;
    }

    if config.quiet {
        return run_quiet_mode(&mut events).await;
    }

    // With `-v` (info) we keep the interactive UI; logs are written to stderr.
    // At `-vv` (debug) or higher, logs would interleave with the rendered UI,
    // so we fall back to a plain, log-friendly event stream.
    if config.verbose >= 2 {
        return run_log_mode(&mut events).await;
    }

    run_interactive_mode(config, &mut events, supports_color).await
}

/// A plain, log-friendly event stream used when verbose logging is enabled.
/// Everything is written to stderr so it can safely share the terminal with
/// `tracing` output.
async fn run_log_mode(events: &mut broadcast::Receiver<TunnelEvent>) -> anyhow::Result<()> {
    loop {
        match events.recv().await {
            Ok(TunnelEvent::Connected { session }) => {
                eprintln!("Connected: {}", session.public_url);
            }
            Ok(TunnelEvent::Connecting { endpoint }) => {
                eprintln!("Connecting to {}...", endpoint);
            }
            Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                eprintln!(
                    "Reconnecting (attempt {}) in {}ms...",
                    attempt,
                    delay.as_millis()
                );
            }
            Ok(event @ TunnelEvent::RequestHandled { .. }) => {
                if let Some(entry) = log::RequestLogEntry::from_event(&event) {
                    eprintln!("{}", entry.format_line(24));
                    let _ = io::stderr().flush();
                }
            }
            Ok(TunnelEvent::Disconnected { reason, graceful }) => {
                if graceful {
                    if reason != "cancelled" && !reason.is_empty() {
                        eprintln!("{}", reason);
                    }
                    return Ok(());
                }
                if !reason.is_empty() {
                    let view = format_disconnect(&reason);
                    eprintln!("{}", view);
                    return Err(FatalTunnelError.into());
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return Ok(()),
        }
    }
}

async fn run_json_mode(events: &mut broadcast::Receiver<TunnelEvent>) -> anyhow::Result<()> {
    loop {
        match events.recv().await {
            Ok(TunnelEvent::Connected { session }) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "connected",
                        "url": session.public_url,
                        "subdomain": session.subdomain,
                    })
                );
            }
            Ok(TunnelEvent::Disconnected { reason, graceful }) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "disconnected",
                        "reason": reason,
                        "graceful": graceful,
                    })
                );
                if graceful {
                    return Ok(());
                }
                if !reason.is_empty() {
                    return Err(FatalTunnelError.into());
                }
            }
            Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "reconnecting",
                        "attempt": attempt,
                        "delay_ms": delay.as_millis(),
                    })
                );
            }
            Ok(TunnelEvent::Connecting { endpoint }) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "connecting",
                        "endpoint": endpoint,
                    })
                );
            }
            Ok(TunnelEvent::RequestHandled {
                stream_id,
                method,
                path,
                status,
                duration,
                hint,
            }) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "request_handled",
                        "stream_id": stream_id,
                        "method": method,
                        "path": path,
                        "status": status,
                        "duration_ms": duration.as_millis(),
                        "hint": hint,
                    })
                );
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return Ok(()),
        }
    }
}

async fn run_quiet_mode(events: &mut broadcast::Receiver<TunnelEvent>) -> anyhow::Result<()> {
    loop {
        match events.recv().await {
            Ok(TunnelEvent::Connected { session }) => {
                eprintln!("{}", session.public_url);
            }
            Ok(TunnelEvent::Disconnected { reason, graceful }) => {
                if graceful {
                    if !reason.is_empty() && reason != "cancelled" {
                        eprintln!("{}", reason);
                    }
                    return Ok(());
                }
                if !reason.is_empty() {
                    eprintln!("{}", reason);
                    return Err(FatalTunnelError.into());
                }
            }
            Ok(TunnelEvent::Connecting { endpoint }) => {
                eprintln!("Connecting to {}...", endpoint);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return Ok(()),
            _ => {}
        }
    }
}

async fn run_interactive_mode(
    config: &Cli,
    events: &mut broadcast::Receiver<TunnelEvent>,
    supports_color: bool,
) -> anyhow::Result<()> {
    let mut banner_printed = false;
    let mut graceful_exit = false;

    loop {
        match events.recv().await {
            Ok(TunnelEvent::Connected { session }) => {
                print_banner(&session, config, supports_color);
                banner_printed = true;
                if let Some(qr) = qr_output(config, &session, supports_color) {
                    eprintln!("Scan with your phone:\n");
                    println!("{}", qr);
                }
                eprintln!("\nPress Ctrl+C to stop sharing.\n");
                eprintln!("Recent Requests:");
            }
            Ok(TunnelEvent::Connecting { endpoint }) if !banner_printed => {
                let status = banner::ConnectionStatus::Connecting;
                eprintln!("  {} ({})", banner::styled_label(status), endpoint);
            }
            Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                let status = banner::ConnectionStatus::Reconnecting { attempt };
                eprintln!(
                    "  {} in {}ms",
                    banner::styled_label(status),
                    delay.as_millis()
                );
            }
            Ok(event @ TunnelEvent::RequestHandled { .. }) => {
                if let Some(entry) = log::RequestLogEntry::from_event(&event) {
                    println!("{}", entry.format_line(24));
                    let _ = io::stdout().flush();
                    if let Some(hint) = entry.format_hint() {
                        eprintln!("{hint}");
                        let _ = io::stderr().flush();
                    }
                }
            }
            Ok(TunnelEvent::Disconnected { reason, graceful }) => {
                if graceful {
                    // "cancelled" is the normal Ctrl+C/SIGTERM path: break out so
                    // the "Stopping localshare..." message below is printed once
                    // the tunnel has fully finished its teardown.
                    graceful_exit = true;
                    if reason != "cancelled" && !reason.is_empty() {
                        eprintln!("Stopping: {}", reason);
                    }
                    break;
                } else if !reason.is_empty() {
                    let view = format_disconnect(&reason);
                    eprintln!("{}", view);
                    return Err(FatalTunnelError.into());
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
            _ => {}
        }
    }

    // Only a user-initiated shutdown (or a clean relay close) prints this line;
    // a broken event channel drops out silently.
    if graceful_exit {
        eprintln!("Stopping localshare...");
    }
    Ok(())
}

/// Build a beautifully rendered, actionable error view for a fatal tunnel
/// disconnect based on the reason string emitted by the tunnel runner. Every
/// classified failure gets a concrete, actionable hint.
fn format_disconnect(reason: &str) -> error_view::ErrorView<'static> {
    let lower = reason.to_ascii_lowercase();

    let (title, hint) = if lower.contains("subdomain") || lower.contains("taken") {
        (
            "Registration rejected",
            Some("Try a different subdomain with -s <name>."),
        )
    } else if lower.contains("serverfull")
        || lower.contains("server full")
        || lower.contains("at capacity")
    {
        (
            "Relay is at capacity",
            Some("The relay has no free slots right now. Wait a moment and retry, or point -r at a different relay."),
        )
    } else if lower.contains("unsupportedclient")
        || lower.contains("client version")
        || lower.contains("version not supported")
    {
        (
            "Client version rejected",
            Some("This version of localshare is not supported by the relay. Update it with: cargo install localshare --force."),
        )
    } else if lower.contains("tls") || lower.contains("ssl") || lower.contains("certificate") {
        (
            "Could not verify relay certificate",
            Some("The relay's TLS certificate could not be verified. Check that -r uses wss:// and points at a trusted relay."),
        )
    } else if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("handshake")
    {
        (
            "Could not reach relay",
            Some("The relay did not respond in time. Check your network connection and that a firewall is not blocking outbound WebSocket traffic."),
        )
    } else if lower.contains("name or service not known")
        || lower.contains("no such host is known")
        || lower.contains("nodename")
        || lower.contains("lookup")
    {
        (
            "Could not resolve relay",
            Some("The relay host name could not be resolved. Verify the address passed to -r/--relay, e.g. -r relay.localshare.dev."),
        )
    } else if lower.contains("refused")
        || lower.contains("could not connect")
        || lower.contains("not reachable")
        || lower.contains("no route to host")
        || lower.contains("unreachable")
    {
        (
            "Could not reach relay",
            Some("Check the relay address (-r) and your network connection."),
        )
    } else if lower.contains("invalid relay")
        || lower.contains("unsupported scheme")
        || lower.contains("missing host")
    {
        (
            "Invalid relay address",
            Some("Use a bare hostname or a ws:// / wss:// URL."),
        )
    } else if lower.contains("registration") {
        (
            "Registration rejected",
            Some("The relay denied your client. Check the subdomain and your client version."),
        )
    } else {
        ("Tunnel disconnected", None)
    };

    match hint {
        Some(hint) => error_view::ErrorView::with_hint(title, "", reason, hint),
        None => error_view::ErrorView::new(title, "", reason),
    }
}

fn print_banner(session: &TunnelSession, config: &Cli, supports_color: bool) {
    if !supports_color {
        colored::control::set_override(false);
    }
    let forwarding = format!("http://{}", session.subdomain);
    let banner = banner::session_to_banner(
        session,
        &forwarding,
        &config.relay,
        env!("CARGO_PKG_VERSION"),
    );
    println!("{}\n", banner);
}

/// Return the terminal QR code to render for a session, or `None` when QR
/// output is disabled — via `--no-qr`, a non-interactive (piped) stdout, or a
/// session that has no public URL yet. Keeping the decision separate from the
/// print call makes it deterministic and unit-testable.
fn qr_output(
    config: &Cli,
    session: &TunnelSession,
    supports_color: bool,
) -> Option<qr::TerminalQr> {
    if config.no_qr || !supports_color || session.public_url.is_empty() {
        return None;
    }
    Some(qr::render_qr(&session.public_url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn hint_of(reason: &str) -> Option<String> {
        format_disconnect(reason).hint
    }

    fn assert_hint(reason: &str, needle: &str) {
        let hint = hint_of(reason).unwrap_or_else(|| panic!("no hint for: {reason}"));
        assert!(
            hint.contains(needle),
            "hint for {reason:?} should mention {needle:?}, got: {hint:?}"
        );
    }

    #[test]
    fn hint_for_refused_connection_suggests_relay_address() {
        assert_hint(
            "connection to relay ws://127.0.0.1:1/ failed: IO error: Connection refused",
            "Check the relay address (-r)",
        );
    }

    #[test]
    fn hint_for_handshake_timeout_suggests_network_and_firewall() {
        assert_hint(
            "handshake with relay ws://relay.localshare.dev/ timed out after 5s",
            "firewall",
        );
    }

    #[test]
    fn hint_for_dns_failure_suggests_verifying_relay_flag() {
        assert_hint(
            "connection to relay relay.example.com:80 failed: failed to lookup address information: Name or service not known",
            "-r/--relay",
        );
    }

    #[test]
    fn hint_for_subdomain_taken_suggests_different_subdomain() {
        assert_hint(
            "registration rejected by relay: SubdomainTaken",
            "different subdomain",
        );
    }

    #[test]
    fn hint_for_invalid_url_suggests_ws_scheme() {
        assert_hint(
            "invalid relay URL 'ftp://relay.example.com': unsupported scheme 'ftp'",
            "ws:// / wss://",
        );
    }

    #[test]
    fn hint_for_server_full_suggests_retry_or_other_relay() {
        assert_hint(
            "registration rejected by relay: ServerFull",
            "no free slots",
        );
    }

    #[test]
    fn hint_for_unsupported_client_suggests_update() {
        assert_hint(
            "registration rejected by relay: UnsupportedClient",
            "cargo install localshare",
        );
    }

    #[test]
    fn hint_for_tls_failure_suggests_wss() {
        assert_hint(
            "connection to relay wss://relay.example.com/ failed: TLS handshake failure: invalid peer certificate",
            "wss://",
        );
    }

    #[test]
    fn hint_for_network_unreachable_suggests_network() {
        assert_hint(
            "connection to relay ws://10.0.0.1/ failed: Network is unreachable",
            "Check the relay address",
        );
    }

    #[test]
    fn unknown_reasons_still_render_message_without_hint() {
        let view = format_disconnect("some opaque failure");
        assert!(view.hint.is_none());
        assert!(view.to_string().contains("some opaque failure"));
    }

    // ── QR output gating ──────────────────────────────────────────────────────

    fn session_with(url: &str) -> TunnelSession {
        TunnelSession {
            subdomain: "demo".into(),
            public_url: url.into(),
            heartbeat_interval_ms: 60_000,
        }
    }

    fn cli_with(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("localshare").chain(args.iter().copied()))
    }

    #[test]
    fn qr_suppressed_with_no_qr_flag() {
        let config = cli_with(&["3000", "--no-qr"]);
        let qr = qr_output(&config, &session_with("https://demo.localshare.dev"), true);
        assert!(qr.is_none(), "--no-qr must suppress the QR matrix");
    }

    #[test]
    fn qr_suppressed_when_stdout_is_not_a_terminal() {
        let config = cli_with(&["3000"]);
        let qr = qr_output(&config, &session_with("https://demo.localshare.dev"), false);
        assert!(qr.is_none(), "piped output must suppress the QR matrix");
    }

    #[test]
    fn qr_suppressed_for_session_without_public_url() {
        let config = cli_with(&["3000"]);
        let qr = qr_output(&config, &session_with(""), true);
        assert!(qr.is_none(), "empty public_url must suppress the QR matrix");
    }

    #[test]
    fn qr_rendered_when_enabled_on_a_terminal() {
        let config = cli_with(&["3000"]);
        let qr = qr_output(&config, &session_with("https://demo.localshare.dev"), true)
            .expect("QR should be rendered on a terminal without --no-qr");
        let text = qr.to_string();
        assert!(
            text.contains('█') || text.contains('▀') || text.contains('▄'),
            "QR should contain block characters, got: {text:?}"
        );
    }
}
