//! yrem: remote-only CLI and MCP server for `yantrikdb-server`.

pub mod cli;
pub mod client;
pub mod config;
pub mod error;
pub mod mcp;
pub mod ops;
pub mod output;
pub mod time;

use std::process::ExitCode;

pub use error::{Error, Result};

/// Entry point: logging to stderr (`RUST_LOG`, default `warn`), then the CLI.
pub async fn run() -> ExitCode {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
    cli::run().await
}
