use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::signal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub node: NodeCommands,
}

#[derive(Subcommand, Debug)]
pub enum NodeCommands {
    /// Run as RSU (Roadside Unit)
    Rsu(rsu_lib::RsuArgs),
    /// Run as OBU (On-Board Unit)
    Obu(obu_lib::ObuArgs),
    /// Run as Server (UDP receiver)
    Server(server_lib::ServerArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    match args.node {
        NodeCommands::Rsu(rsu_args) => {
            let _node = rsu_lib::create(rsu_args)?;
            let _ = signal::ctrl_c().await;
        }
        NodeCommands::Obu(obu_args) => {
            let _node = obu_lib::create(obu_args)?;
            let _ = signal::ctrl_c().await;
        }
        NodeCommands::Server(server_args) => {
            let _server = server_lib::create(server_args).await?;
            tracing::info!("Server started successfully. Press Ctrl+C to stop.");
            let _ = signal::ctrl_c().await;
        }
    }

    Ok(())
}
