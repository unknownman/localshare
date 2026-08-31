// The public library API is not yet consumed by main(); suppress dead-code
// lints until CLI wiring is complete in a subsequent task.
#![allow(dead_code, unused_imports)]

mod cli;
mod error;
mod tunnel;

use clap::Parser;

#[tokio::main]
async fn main() {
    let _cli = cli::Cli::parse();
}
