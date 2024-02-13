use std::net::Ipv4Addr;

use clap::Parser;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Interface to bind to
    #[arg(short, long)]
    pub bind: String,

    /// Virtual device name
    #[arg(short, long)]
    pub tap_name: Option<String>,

    /// Node identifier
    #[arg(short, long)]
    pub uuid: Option<Uuid>,

    /// IP
    #[arg(short, long)]
    pub ip: Option<Ipv4Addr>,
}
