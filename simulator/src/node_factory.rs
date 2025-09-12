use anyhow::Result;
use common::{device::Device, tun::Tun};
use std::{any::Any, sync::Arc, str::FromStr};

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

/// Factory function to create nodes based on configuration
/// Returns either Arc<rsu_lib::Rsu> or Arc<obu_lib::Obu> wrapped in Any
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
) -> Result<Arc<dyn Any + Send + Sync>> {
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
            
            let rsu_node = rsu_lib::create_with_vdev(rsu_args, tun, device)?;
            // Extract the concrete RSU from the trait object
            let rsu_any = rsu_node.as_any();
            let rsu_concrete = rsu_any.downcast_ref::<rsu_lib::Rsu>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast RSU"))?;
            Ok(Arc::new(rsu_concrete.clone()) as Arc<dyn Any + Send + Sync>)
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
            
            let obu_node = obu_lib::create_with_vdev(obu_args, tun, device)?;
            // Extract the concrete OBU from the trait object
            let obu_any = obu_node.as_any();
            let obu_concrete = obu_any.downcast_ref::<obu_lib::Obu>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast OBU"))?;
            Ok(Arc::new(obu_concrete.clone()) as Arc<dyn Any + Send + Sync>)
        }
    }
}