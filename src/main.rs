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

    let events = run_tunnel(config, cancel.clone()).await;

    let ctrl_c_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        ctrl_c_cancel.cancel();
    });

    match run_ui(&cli, events, cancel).await {
        Ok(()) => 0,
        Err(e) if e.downcast_ref::<ui::FatalTunnelError>().is_some() => 1,
        Err(e) => {
            use colored::Colorize;
            eprintln!("{} {}", "error:".red().bold(), e);
            1
        }
    }
}

/// Configure `tracing_subscriber` based on the `-v` verbosity flag.
///
/// `RUST_LOG` takes precedence when set; otherwise verbosity maps to:
/// 0 → `error`, 1 (single `-v`) → `info`, 2 (`-vv`) → `debug`, 3+ → `trace`.
/// Logs are written to stderr so they never corrupt stdout output (JSON mode,
/// the QR code, or the interactive request log).
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
