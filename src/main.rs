//! `localshare` — instantly share a local HTTP server with the internet.
//!
//! A binary crate that wires together three layers:
//!
//! - [`cli`] parses and validates command-line arguments.
//! - [`tunnel`] runs the relay WebSocket connection, reconnect logic, and the
//!   local HTTP forwarding engine.
//! - [`ui`] renders tunnel events for humans (interactive banner + QR + request
//!   log) or scripts (JSON), and translates fatal errors into exit codes.
//!
//! Graceful shutdown is signal-driven: on Unix, `SIGINT` and `SIGTERM` cancel
//! the tunnel (which flushes `Unregister` to the relay), and the process exits
//! with code 0 once the WebSocket is fully closed.

use clap::Parser;
use colored::control::set_override;
use std::io::IsTerminal;
use tokio_util::sync::CancellationToken;

mod cli;
mod error;
mod tunnel;
mod ui;

use crate::tunnel::client::{run_tunnel, TunnelConfig};
use crate::ui::run_ui;

#[tokio::main]
async fn main() {
    let code = run().await;
    std::process::exit(code);
}

async fn run() -> i32 {
    let cli = cli::Cli::parse();

    if std::env::var("NO_COLOR").is_ok() || !std::io::stdout().is_terminal() {
        set_override(false);
    }

    init_tracing(cli.verbose);

    let config = TunnelConfig {
        relay: cli.relay.clone(),
        requested_subdomain: cli.subdomain.clone(),
        target: cli.target.clone(),
        ..Default::default()
    };

    let cancel = CancellationToken::new();

    let events = if std::env::var("LOCALSHARE_DEMO_MODE").is_ok() {
        run_demo_mode(config, cancel.clone())
    } else {
        run_tunnel(config, cancel.clone()).await
    };

    // Failures to install signal handlers happen before the UI loop starts;
    // surface them as a clean, coloured error rather than a panic.
    if let Err(e) = install_shutdown_listener(cancel.clone()) {
        print_error(&e);
        return 1;
    }

    match run_ui(&cli, events).await {
        Ok(()) => 0,
        Err(e) if e.downcast_ref::<ui::FatalTunnelError>().is_some() => 1,
        Err(e) => {
            print_error(&e);
            1
        }
    }
}

/// Render an error to stderr with a consistent `error:` prefix.
fn print_error(e: &dyn std::fmt::Display) {
    use colored::Colorize;
    eprintln!("{} {}", "error:".red().bold(), e);
}

/// Forward OS shutdown signals into the tunnel's cancellation token so the
/// client can send `Unregister` and exit gracefully.
///
/// On Unix, both `SIGINT` (Ctrl+C) and `SIGTERM` (Docker/systemd) trigger a
/// graceful shutdown. On Windows only `Ctrl+C` is supported by Tokio.
#[cfg(unix)]
fn install_shutdown_listener(cancel: CancellationToken) -> std::io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => cancel.cancel(),
            _ = sigterm.recv() => cancel.cancel(),
        }
    });

    Ok(())
}

#[cfg(not(unix))]
fn install_shutdown_listener(cancel: CancellationToken) -> std::io::Result<()> {
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel.cancel();
    });

    Ok(())
}

/// Configure `tracing_subscriber` based on the `-v` verbosity flag.
///
/// `RUST_LOG` takes precedence when set; otherwise verbosity maps to:
/// 0 → `error`, 1 (single `-v`) → `info`, 2 (`-vv`) → `debug`, 3+ → `trace`.
/// Logs are written to stderr so they never corrupt stdout output (JSON mode,
/// the QR code, or the interactive request log).
/// Hidden mock event generator for VHS demo recordings.
///
/// Activated when `LOCALSHARE_DEMO_MODE` is set. Emits a deterministic sequence
/// of `TunnelEvent`s that populate the UI banner and live request log without
/// requiring a real relay connection or internet access.
fn run_demo_mode(
    config: TunnelConfig,
    cancel: CancellationToken,
) -> tokio::sync::broadcast::Receiver<crate::tunnel::client::TunnelEvent> {
    use crate::tunnel::client::{TunnelEvent, TunnelSession};
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let subdomain = config
        .requested_subdomain
        .unwrap_or_else(|| "demo".to_string());
    let public_url = format!("https://{}.relay.localshare.dev", subdomain);

    tokio::spawn(async move {
        // 1. Simulate connection delay
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let _ = tx.send(TunnelEvent::Connected {
            session: TunnelSession {
                subdomain,
                public_url,
                heartbeat_interval_ms: 60_000,
            },
        });

        // 2. Simulate a GET request
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let _ = tx.send(TunnelEvent::RequestHandled {
            stream_id: 1,
            method: "GET".into(),
            path: "/".into(),
            status: 200,
            duration: std::time::Duration::from_millis(14),
            hint: None,
        });

        // 3. Simulate a POST webhook
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let _ = tx.send(TunnelEvent::RequestHandled {
            stream_id: 2,
            method: "POST".into(),
            path: "/api/webhooks/stripe".into(),
            status: 200,
            duration: std::time::Duration::from_millis(42),
            hint: None,
        });

        // 4. Simulate a 404 error
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        let _ = tx.send(TunnelEvent::RequestHandled {
            stream_id: 3,
            method: "GET".into(),
            path: "/favicon.ico".into(),
            status: 404,
            duration: std::time::Duration::from_millis(2),
            hint: None,
        });

        // 5. Wait for the Ctrl+C signal from the tape
        cancel.cancelled().await;
        let _ = tx.send(TunnelEvent::Disconnected {
            reason: "cancelled".into(),
            graceful: true,
        });
    });

    rx
}

fn init_tracing(verbose: u8) {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

    let level = match verbose {
        0 => "error",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .ok()
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new(level));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(false))
        .try_init();
}
