use anyhow::Result;
use clap::Parser;
use node_lib::args::Args;
use tokio::signal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let _ = node_lib::create(args);
    let _ = signal::ctrl_c().await;
    Ok(())
}
