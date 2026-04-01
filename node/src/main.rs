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
    /// Generate an Ed25519 signing keypair for DH message authentication.
    /// Prints the seed (for signing_key_seed in node YAML) and the verifying
    /// key (for dh_signing_allowlist on the server or server_signing_pubkey on
    /// an OBU).  Uses a cryptographically secure random number generator.
    Keygen,
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
        NodeCommands::Keygen => {
            let kp = node_lib::crypto::SigningKeypair::generate();
            let seed_hex: String = kp.seed_bytes().iter().map(|b| format!("{b:02x}")).collect();
            let pubkey_hex: String = kp
                .verifying_key_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            println!("Ed25519 signing keypair for DH authentication");
            println!();
            println!("Seed (signing_key_seed in node YAML — keep secret):");
            println!("  {seed_hex}");
            println!();
            println!("Verifying key (for dh_signing_allowlist on server, or server_signing_pubkey on OBU):");
            println!("  {pubkey_hex}");
        }
    }

    Ok(())
}
