use anyhow::Result;
use clap::{Parser, ValueEnum};
use tokio::signal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum NodeType {
    Rsu,
    Obu,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct NodeArgs {
    /// Node type to create
    #[arg(short = 't', long, value_enum)]
    pub node_type: NodeType,

    /// Interface to bind to
    #[arg(short, long)]
    pub bind: String,

    /// Virtual device name
    #[arg(long)]
    pub tap_name: Option<String>,

    /// IP
    #[arg(short, long)]
    pub ip: Option<std::net::Ipv4Addr>,

    /// MTU
    #[arg(short, long, default_value_t = 1436)]
    pub mtu: i32,

    /// Hello history
    #[arg(long, default_value_t = 10)]
    pub hello_history: u32,

    /// Hello periodicity (required for RSU)
    #[arg(long)]
    pub hello_periodicity: Option<u32>,

    /// Number of cached upstream candidates to keep for fast failover
    #[arg(long, default_value_t = 3)]
    pub cached_candidates: u32,

    /// Enable payload encryption between OBUs and upstream RSUs
    #[arg(long, default_value_t = false)]
    pub enable_encryption: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = NodeArgs::parse();
    
    match args.node_type {
        NodeType::Rsu => {
            let hello_periodicity = args.hello_periodicity
                .ok_or_else(|| anyhow::anyhow!("RSU requires hello_periodicity to be specified"))?;
            
            let rsu_args = rsu_lib::RsuArgs {
                bind: args.bind,
                tap_name: args.tap_name,
                ip: args.ip,
                mtu: args.mtu,
                rsu_params: rsu_lib::RsuParameters {
                    hello_history: args.hello_history,
                    hello_periodicity,
                    cached_candidates: args.cached_candidates,
                    enable_encryption: args.enable_encryption,
                },
            };
            
            let _node = rsu_lib::create(rsu_args)?;
        }
        NodeType::Obu => {
            let obu_args = obu_lib::ObuArgs {
                bind: args.bind,
                tap_name: args.tap_name,
                ip: args.ip,
                mtu: args.mtu,
                obu_params: obu_lib::ObuParameters {
                    hello_history: args.hello_history,
                    cached_candidates: args.cached_candidates,
                    enable_encryption: args.enable_encryption,
                },
            };
            
            let _node = obu_lib::create(obu_args)?;
        }
    }
    
    let _ = signal::ctrl_c().await;
    Ok(())
}
