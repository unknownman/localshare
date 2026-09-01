use crate::cli::Cli;
use crate::tunnel::client::{TunnelEvent, TunnelSession};
use std::io::{self, IsTerminal, Write};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod banner;
pub mod error_view;
pub mod log;
pub mod qr;

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
        run_json_mode(&mut events, cancel).await?;
        return Ok(());
    }

    if config.quiet {
        run_quiet_mode(&mut events, cancel).await?;
        return Ok(());
    }

    run_interactive_mode(config, &mut events, cancel, supports_color).await
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
                    Ok(TunnelEvent::RequestHandled { stream_id, method, path, status, duration }) => {
                        println!("{}", serde_json::json!({
                            "event": "request_handled",
                            "stream_id": stream_id,
                            "method": method,
                            "path": path,
                            "status": status,
                            "duration_ms": duration.as_millis(),
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
                        print_qr(&session, supports_color);
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
                        }
                    }
                    Ok(TunnelEvent::Disconnected { reason }) if !reason.is_empty() && reason != "cancelled" => {
                        let view = error_view::ErrorView::new("Tunnel disconnected", "", &reason);
                        eprintln!("{}", view);
                        break;
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
