use clap::Parser;
use std::net::Ipv4Addr;

#[derive(clap::Args, Clone, Debug)]
pub struct RsuParameters {
    /// Hello history
    #[arg(long, default_value_t = 10)]
    pub hello_history: u32,

    /// Hello periodicity (required for RSU)
    #[arg(short = 'p', long)]
    pub hello_periodicity: u32,

    /// Number of cached upstream candidates to keep for fast failover
    #[arg(long, default_value_t = 3)]
    pub cached_candidates: u32,

    /// Enable payload encryption between OBUs and upstream RSUs
    #[arg(long, default_value_t = false)]
    pub enable_encryption: bool,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct RsuArgs {
    /// Interface to bind to
    #[arg(short, long)]
    pub bind: String,

    /// Virtual device name
    #[arg(short, long)]
    pub tap_name: Option<String>,

    /// IP
    #[arg(short, long)]
    pub ip: Option<Ipv4Addr>,

    /// MTU
    #[arg(short, long, default_value_t = 1436)]
    pub mtu: i32,

    /// RSU Parameters
    #[command(flatten)]
    pub rsu_params: RsuParameters,
}
