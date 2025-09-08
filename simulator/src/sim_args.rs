use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,

    #[arg(short, long, default_value_t = false)]
    pub pretty: bool,

    /// Server address for RSUs to connect to for encrypted traffic processing
    /// If not provided, RSUs will process traffic locally (legacy mode)
    #[arg(long)]
    pub server_address: Option<SocketAddr>,
}
