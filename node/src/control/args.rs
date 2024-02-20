use std::net::Ipv4Addr;

use clap::{Parser, ValueEnum};

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

    /// Node Parameters
    #[command(flatten)]
    pub node_params: NodeParameters,
}
