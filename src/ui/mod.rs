use crate::cli::Cli;
use crate::tunnel::client::{TunnelEvent, TunnelSession};
use std::io::{self, IsTerminal, Write};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

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
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let supports_color = io::stdout().is_terminal();
    if std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }

    if config.json {
        return run_json_mode(&mut events, cancel).await;
    }

    if config.quiet {
        return run_quiet_mode(&mut events, cancel).await;
    }

    // With `-v` (info) we keep the interactive UI; logs are written to stderr.
    // At `-vv` (debug) or higher, logs would interleave with the rendered UI,
    // so we fall back to a plain, log-friendly event stream.
    if config.verbose >= 2 {
        return run_log_mode(&mut events, cancel).await;
    }

    run_interactive_mode(config, &mut events, cancel, supports_color).await
}

/// A plain, log-friendly event stream used when verbose logging is enabled.
/// Everything is written to stderr so it can safely share the terminal with
/// `tracing` output.
async fn run_log_mode(
    events: &mut broadcast::Receiver<TunnelEvent>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(TunnelEvent::Connected { session }) => {
                        eprintln!("Connected: {}", session.public_url);
                    }
                    Ok(TunnelEvent::Connecting { endpoint }) => {
                        eprintln!("Connecting to {}...", endpoint);
                    }
                    Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                        eprintln!("Reconnecting (attempt {}) in {}ms...", attempt, delay.as_millis());
                    }
                    Ok(event @ TunnelEvent::RequestHandled { .. }) => {
                        if let Some(entry) = log::RequestLogEntry::from_event(&event) {
                            eprintln!("{}", entry.format_line(24));
                            let _ = io::stderr().flush();
                        }
                    }
                    Ok(TunnelEvent::Disconnected { reason }) if !reason.is_empty() && reason != "cancelled" => {
                        let view = format_disconnect(&reason);
                        eprintln!("{}", view);
                        return Err(FatalTunnelError.into());
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

async fn run_json_mode(
    events: &mut broadcast::Receiver<TunnelEvent>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(TunnelEvent::Connected { session }) => {
                        println!("{}", serde_json::json!({
                            "event": "connected",
                            "url": session.public_url,
                            "subdomain": session.subdomain,
                        }));
                    }
                    Ok(TunnelEvent::Disconnected { reason }) => {
                        println!("{}", serde_json::json!({
                            "event": "disconnected",
                            "reason": reason,
                        }));
                        if !reason.is_empty() && reason != "cancelled" {
                            return Err(FatalTunnelError.into());
                        }
                    }
                    Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                        println!("{}", serde_json::json!({
                            "event": "reconnecting",
                            "attempt": attempt,
                            "delay_ms": delay.as_millis(),
                        }));
                    }
                    Ok(TunnelEvent::Connecting { endpoint }) => {
                        println!("{}", serde_json::json!({
                            "event": "connecting",
                            "endpoint": endpoint,
                        }));
                    }
                    Ok(TunnelEvent::RequestHandled { stream_id, method, path, status, duration, hint }) => {
                        println!("{}", serde_json::json!({
                            "event": "request_handled",
                            "stream_id": stream_id,
                            "method": method,
                            "path": path,
                            "status": status,
                            "duration_ms": duration.as_millis(),
                            "hint": hint,
                        }));
                    }
                    Err(_) => break,
                }
            }
        }
    }
    Ok(())
}

async fn run_quiet_mode(
    events: &mut broadcast::Receiver<TunnelEvent>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(TunnelEvent::Connected { session }) => {
                        eprintln!("{}", session.public_url);
                    }
                    Ok(TunnelEvent::Disconnected { reason }) => {
                        if !reason.is_empty() {
                            eprintln!("{}", reason);
                            if reason != "cancelled" {
                                return Err(FatalTunnelError.into());
                            }
                        }
                    }
                    Ok(TunnelEvent::Connecting { endpoint }) => {
                        eprintln!("Connecting to {}...", endpoint);
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

async fn run_interactive_mode(
    config: &Cli,
    events: &mut broadcast::Receiver<TunnelEvent>,
    cancel: CancellationToken,
    supports_color: bool,
) -> anyhow::Result<()> {
    let mut banner_printed = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(TunnelEvent::Connected { session }) => {
                        print_banner(&session, config, supports_color);
                        banner_printed = true;
                        if !config.no_qr {
                            print_qr(&session, supports_color);
                        }
                        eprintln!("\nPress Ctrl+C to stop sharing.\n");
                        eprintln!("Recent Requests:");
                    }
                    Ok(TunnelEvent::Connecting { endpoint }) if !banner_printed => {
                        let (label, color) = banner::ConnectionStatus::Connecting.as_label();
                        eprintln!("  \x1b[1m{}{}\x1b[0m ({})", color, label, endpoint);
                    }
                    Ok(TunnelEvent::Reconnecting { attempt, delay }) => {
                        let (label, color) = banner::ConnectionStatus::Reconnecting { attempt }.as_label();
                        eprintln!("  \x1b[1m{}{}\x1b[0m in {}ms", color, label, delay.as_millis());
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
                    Ok(TunnelEvent::Disconnected { reason }) if !reason.is_empty() && reason != "cancelled" => {
                        let view = format_disconnect(&reason);
                        eprintln!("{}", view);
                        return Err(FatalTunnelError.into());
                    }
                    _ => {}
                }
            }
        }
    }

    if banner_printed {
        eprintln!("Stopping localshare...");
    }
    Ok(())
}

/// Build a beautifully rendered, actionable error view for a fatal tunnel
/// disconnect based on the reason string emitted by the tunnel runner.
fn format_disconnect(reason: &str) -> error_view::ErrorView<'static> {
    let lower = reason.to_ascii_lowercase();

    let (title, hint) = if lower.contains("subdomain") || lower.contains("taken") {
        (
            "Registration rejected",
            Some("Try a different subdomain with -s <name>."),
        )
    } else if lower.contains("refused")
        || lower.contains("could not connect")
        || lower.contains("not reachable")
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

fn print_qr(session: &TunnelSession, supports_color: bool) {
    if !supports_color || session.public_url.is_empty() {
        return;
    }
    eprintln!("Scan with your phone:\n");
    let qr = qr::render_qr(&session.public_url);
    println!("{}", qr);
}
