use anyhow::Result;
use clap::Parser;
use node::{args::Args, create};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let mut args = Args::parse();
    args.uuid = Some(Uuid::new_v4());

    create(args).await
}
