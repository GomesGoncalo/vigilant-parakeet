use clap::Parser;
use std::net::Ipv4Addr;

#[derive(clap::Args, Clone, Debug)]
pub struct ServerParameters {
    /// UDP port to listen on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct ServerArgs {
    /// IP address to bind to
    #[arg(short, long)]
    pub ip: Ipv4Addr,

    /// Server Parameters
    #[command(flatten)]
    pub server_params: ServerParameters,
}
