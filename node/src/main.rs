use anyhow::Result;
use clap::Parser;
use node::control::args::Args;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    node::create(args).await
}
