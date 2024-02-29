use clap::{Parser, ValueEnum};
use std::net::Ipv4Addr;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum NodeType {
    Rsu,
    Obu,
}

#[derive(clap::Args, Clone, Debug)]
#[group(required = true, multiple = false)]
pub struct NodeParameters {
    /// Node type
    #[arg(short, long)]
    pub node_type: NodeType,

    /// Hello history
    #[arg(short, long, default_value_t = 10)]
    pub hello_history: u32,

    /// Hello periodicity
    #[arg(short, long)]
    pub hello_periodicity: Option<u32>,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
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
    #[arg(short, long, default_value_t = 1465)]
    pub mtu: i32,

    /// Node Parameters
    #[command(flatten)]
    pub node_params: NodeParameters,
}
