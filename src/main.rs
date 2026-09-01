use clap::Parser;
use colored::control::set_override;
use std::io::IsTerminal;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cli;
mod error;
mod tunnel;
mod ui;

use crate::tunnel::client::{run_tunnel, TunnelConfig};
use crate::ui::run_ui;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        use colored::Colorize;
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    if std::env::var("NO_COLOR").is_ok() || !std::io::stdout().is_terminal() {
        set_override(false);
    }

    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    let _ = subscriber.try_init();

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
        if let Ok(()) = tokio::signal::ctrl_c().await {
            ctrl_c_cancel.cancel();
        }
    });

    run_ui(&cli, events, cancel).await
}
