use anyhow::Result;
use common::{device::Device, tun::Tun};
use obu_lib::Node as ObuNode;
use rsu_lib::Node as RsuNode;
use std::{any::Any, str::FromStr, sync::Arc};

/// Node type enum for configuration
#[derive(Clone, Debug, PartialEq)]
pub enum NodeType {
    Rsu,
    Obu,
}

impl FromStr for NodeType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rsu" => Ok(NodeType::Rsu),
            "obu" => Ok(NodeType::Obu),
            _ => Err(anyhow::anyhow!("Invalid node type: {}", s)),
        }
    }
}

/// Unified node enum that can hold either RSU or OBU
#[derive(Clone)]
pub enum UnifiedNode {
    Rsu(Arc<rsu_lib::Rsu>),
    Obu(Arc<obu_lib::Obu>),
}

impl UnifiedNode {
    pub fn as_any(&self) -> &dyn Any {
        match self {
            UnifiedNode::Rsu(rsu) => rsu.as_any(),
            UnifiedNode::Obu(obu) => obu.as_any(),
        }
    }

    pub fn is_rsu(&self) -> bool {
        matches!(self, UnifiedNode::Rsu(_))
    }

    pub fn is_obu(&self) -> bool {
        matches!(self, UnifiedNode::Obu(_))
    }

    pub fn cached_upstream_route(&self) -> Option<obu_lib::control::route::Route> {
        match self {
            UnifiedNode::Obu(obu) => obu.cached_upstream_route(),
            UnifiedNode::Rsu(_) => None, // RSUs don't have upstream routes
        }
    }

    pub fn node_type_name(&self) -> &'static str {
        match self {
            UnifiedNode::Rsu(_) => "Rsu",
            UnifiedNode::Obu(_) => "Obu",
        }
    }
}

/// Factory function to create nodes based on configuration
pub fn create_node_with_vdev(
    node_type: NodeType,
    bind: String,
    tap_name: Option<String>,
    ip: Option<std::net::Ipv4Addr>,
    mtu: i32,
    hello_history: u32,
    hello_periodicity: Option<u32>,
    cached_candidates: u32,
    enable_encryption: bool,
    tun: Arc<Tun>,
    device: Arc<Device>,
) -> Result<UnifiedNode> {
    match node_type {
        NodeType::Rsu => {
            let hello_periodicity = hello_periodicity
                .ok_or_else(|| anyhow::anyhow!("RSU requires hello_periodicity"))?;

            let rsu_args = rsu_lib::RsuArgs {
                bind,
                tap_name,
                ip,
                mtu,
                rsu_params: rsu_lib::RsuParameters {
                    hello_history,
                    hello_periodicity,
                    cached_candidates,
                    enable_encryption,
                },
            };

            // Create RSU directly and wrap it
            let rsu = rsu_lib::Rsu::new(rsu_args, tun, device)?;
            Ok(UnifiedNode::Rsu(rsu))
        }
        NodeType::Obu => {
            let obu_args = obu_lib::ObuArgs {
                bind,
                tap_name,
                ip,
                mtu,
                obu_params: obu_lib::ObuParameters {
                    hello_history,
                    cached_candidates,
                    enable_encryption,
                },
            };

            // Create OBU directly and wrap it
            let obu = obu_lib::Obu::new(obu_args, tun, device)?;
            Ok(UnifiedNode::Obu(obu))
        }
    }
}
