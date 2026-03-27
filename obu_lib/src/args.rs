use clap::Parser;
use std::net::Ipv4Addr;

#[derive(clap::Args, Clone, Debug)]
pub struct ObuParameters {
    /// Hello history
    #[arg(long, default_value_t = 10)]
    pub hello_history: u32,

    /// Number of cached upstream candidates to keep for fast failover
    #[arg(long, default_value_t = 3)]
    pub cached_candidates: u32,

    /// Enable payload encryption between OBUs and upstream RSUs
    #[arg(long, default_value_t = false)]
    pub enable_encryption: bool,

    /// Enable Diffie-Hellman key negotiation (requires enable_encryption)
    #[arg(long, default_value_t = false)]
    pub enable_dh: bool,

    /// Interval in milliseconds between DH re-key exchanges
    #[arg(long, default_value_t = 60_000)]
    pub dh_rekey_interval_ms: u64,

    /// Maximum lifetime of a DH-derived key in milliseconds before forced re-key
    #[arg(long, default_value_t = 120_000)]
    pub dh_key_lifetime_ms: u64,

    /// Number of DH key exchange attempts before giving up and falling back to fixed key
    #[arg(long, default_value_t = 3)]
    pub dh_max_retries: u32,

    /// Timeout in milliseconds to wait for a DH reply before retrying
    #[arg(long, default_value_t = 5_000)]
    pub dh_reply_timeout_ms: u64,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct ObuArgs {
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
    #[arg(short, long, default_value_t = 1400)]
    pub mtu: i32,

    /// OBU Parameters
    #[command(flatten)]
    pub obu_params: ObuParameters,
}
