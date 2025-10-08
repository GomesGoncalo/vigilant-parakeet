//! Network interface organization for simulator nodes
//!
//! This module provides a clean abstraction for managing the multiple network interfaces
//! that each node type requires. All interfaces are kept alive via Arc to prevent premature closure.

use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

use crate::simulator::SimNode;
use std::net::Ipv4Addr;

/// All network interfaces for a node, organized by purpose.
/// All interfaces must be kept alive (Arc) throughout the simulation to prevent closure.
///
/// # Interface Types
///
/// - **VANET**: The wireless medium interface where VANET control and data messages flow (OBU/RSU only)
/// - **Virtual**: TAP interface for decapsulated data traffic (OBU/RSU/Server, 10.x.x.x range)
/// - **Cloud**: Infrastructure/Internet connectivity interface (RSU/Server only, 172.x.x.x range)
///
/// # Node Interface Requirements
///
/// - **OBU**: vanet (required), virtual (required), cloud (none)
/// - **RSU**: vanet (required), virtual (required), cloud (required)
/// - **Server**: vanet (none), virtual (required), cloud (required)
#[derive(Clone)]
pub struct NodeInterfaces {
    /// VANET medium interface (OBU/RSU only, bound to Device)
    /// This is where control/data messages flow over the wireless medium
    pub vanet: Option<Arc<Tun>>,

    /// Virtual TAP interface for decapsulated data traffic (optional for future expansion)
    /// - OBU/RSU: where forwarded data packets are delivered (10.x.x.x)
    /// - Server: where data from the distributed network arrives (10.x.x.x)
    pub virtual_tap: Option<Arc<Tun>>,

    /// Cloud/infrastructure interface (RSU/Server only)
    /// - RSU: UDP socket interface to forward to servers (172.x.x.x)
    /// - Server: UDP listener interface receiving from RSUs (172.x.x.x)
    pub cloud: Option<Arc<Tun>>,
    /// IP address configured on the virtual TAP (10.x.x.x) when present
    #[cfg_attr(not(any(feature = "tui", feature = "webview", feature = "test_helpers")), allow(dead_code))]
    pub virtual_ip: Option<Ipv4Addr>,

    /// IP address configured on the cloud interface (172.x.x.x) when present
    #[cfg_attr(not(any(feature = "tui", feature = "webview", feature = "test_helpers")), allow(dead_code))]
    pub cloud_ip: Option<Ipv4Addr>,
}

impl NodeInterfaces {
    /// Create interfaces for an OBU node
    ///
    /// OBUs have:
    /// - VANET interface for wireless communication
    /// - Virtual interface for decapsulated data traffic
    pub fn obu(vanet: Arc<Tun>, virtual_tap: Arc<Tun>, virtual_ip: Option<Ipv4Addr>) -> Self {
        Self {
            vanet: Some(vanet),
            virtual_tap: Some(virtual_tap),
            cloud: None,
            virtual_ip,
            cloud_ip: None,
        }
    }

    /// Create interfaces for an RSU node
    ///
    /// RSUs have:
    /// - VANET interface for wireless communication
    /// - Virtual interface for decapsulated data traffic
    /// - Cloud interface for forwarding to servers
    pub fn rsu(
        vanet: Arc<Tun>,
        virtual_tap: Arc<Tun>,
        cloud: Arc<Tun>,
        virtual_ip: Option<Ipv4Addr>,
        cloud_ip: Option<Ipv4Addr>,
    ) -> Self {
        Self {
            vanet: Some(vanet),
            virtual_tap: Some(virtual_tap),
            cloud: Some(cloud),
            virtual_ip,
            cloud_ip,
        }
    }

    /// Create interfaces for a Server node
    ///
    /// Servers have:
    /// - Virtual interface for distributed network communication
    /// - Cloud interface for receiving from RSUs
    pub fn server(
        virtual_tap: Arc<Tun>,
        cloud: Arc<Tun>,
        virtual_ip: Option<Ipv4Addr>,
        cloud_ip: Option<Ipv4Addr>,
    ) -> Self {
        Self {
            vanet: None,
            virtual_tap: Some(virtual_tap),
            cloud: Some(cloud),
            virtual_ip,
            cloud_ip,
        }
    }

    // Removed several convenience helpers (interface_count, interface_names, has_*)
    // because they were not used anywhere in the codebase. The user requested
    // unused API be removed rather than silenced; callers should inspect the
    // fields directly when needed.

    /// Get the VANET interface (if present)
    pub fn vanet(&self) -> Option<&Arc<Tun>> {
        self.vanet.as_ref()
    }

    /// Get the virtual interface (if present)
    #[cfg(feature = "test_helpers")]
    pub fn virtual_tap(&self) -> Option<&Arc<Tun>> {
        self.virtual_tap.as_ref()
    }

    /// Get the cloud interface (if present)
    #[cfg(feature = "test_helpers")]
    pub fn cloud(&self) -> Option<&Arc<Tun>> {
        self.cloud.as_ref()
    }

    // Note: accessors for the stored IPs were removed in favor of reading
    // the fields directly where needed (TUI snapshot extraction reads
    // `interfaces.virtual_ip` and `interfaces.cloud_ip`). Keeping these
    // methods was creating dead-code warnings; remove them to avoid
    // unused API surface.

    /// Validate that required interfaces are present for the given node type
    pub fn validate(&self, node_type: &str) -> Result<()> {
        match node_type {
            "Obu" => {
                if self.vanet.is_none() {
                    anyhow::bail!("OBU node missing required VANET interface");
                }
                if self.virtual_tap.is_none() {
                    anyhow::bail!("OBU node missing required virtual interface");
                }
                if self.cloud.is_some() {
                    anyhow::bail!("OBU node should not have cloud interface");
                }
            }
            "Rsu" => {
                if self.vanet.is_none() {
                    anyhow::bail!("RSU node missing required VANET interface");
                }
                if self.virtual_tap.is_none() {
                    anyhow::bail!("RSU node missing required virtual interface");
                }
                if self.cloud.is_none() {
                    anyhow::bail!("RSU node missing required cloud interface");
                }
            }
            "Server" => {
                if self.vanet.is_some() {
                    anyhow::bail!("Server node should not have VANET interface");
                }
                if self.virtual_tap.is_none() {
                    anyhow::bail!("Server node missing required virtual interface");
                }
                if self.cloud.is_none() {
                    anyhow::bail!("Server node missing required cloud interface");
                }
            }
            _ => anyhow::bail!("Unknown node type: {}", node_type),
        }
        Ok(())
    }
}

/// Result of creating a node in its namespace
///
/// This struct encapsulates all the components needed for a node to operate
/// in the simulator, keeping interfaces organized and ownership clear.
pub struct NodeCreationResult {
    /// Device wrapper for the VANET interface (OBU/RSU only)
    /// Server nodes have a dummy device since they don't participate in VANET medium
    pub device: Arc<Device>,

    /// All network interfaces (owned to keep them alive throughout simulation)
    pub interfaces: NodeInterfaces,

    /// The node instance (OBU/RSU/Server)
    pub node: SimNode,
}

impl NodeCreationResult {
    /// Create a new node creation result
    pub fn new(device: Arc<Device>, interfaces: NodeInterfaces, node: SimNode) -> Self {
        Self {
            device,
            interfaces,
            node,
        }
    }

    /// Validate that the interfaces match the node type
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<()> {
        let node_type = match self.node {
            SimNode::Obu(_) => "Obu",
            SimNode::Rsu(_) => "Rsu",
            SimNode::Server(_) => "Server",
        };
        self.interfaces.validate(node_type)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "test_helpers")]
    use super::NodeInterfaces;
    #[cfg(feature = "test_helpers")]
    use std::sync::Arc;

    #[test]
    fn obu_interfaces_valid() {
        #[cfg(feature = "test_helpers")]
        {
            use super::*;
            let (tun1, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun2, _) = node_lib::test_helpers::util::mk_shim_pair();

            let interfaces = NodeInterfaces::obu(Arc::new(tun1), Arc::new(tun2));

            assert!(interfaces.vanet().is_some());
            assert!(interfaces.virtual_tap().is_some());
            assert!(interfaces.cloud().is_none());
            assert!(interfaces.validate("Obu").is_ok());
        }
    }

    #[test]
    fn rsu_interfaces_valid() {
        #[cfg(feature = "test_helpers")]
        {
            let (tun1, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun2, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun3, _) = node_lib::test_helpers::util::mk_shim_pair();

            let interfaces = NodeInterfaces::rsu(Arc::new(tun1), Arc::new(tun2), Arc::new(tun3));

            assert!(interfaces.vanet().is_some());
            assert!(interfaces.virtual_tap().is_some());
            assert!(interfaces.cloud().is_some());
            assert!(interfaces.validate("Rsu").is_ok());
        }
    }

    #[test]
    fn server_interfaces_valid() {
        #[cfg(feature = "test_helpers")]
        {
            let (tun1, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun2, _) = node_lib::test_helpers::util::mk_shim_pair();

            let interfaces = NodeInterfaces::server(Arc::new(tun1), Arc::new(tun2));

            assert!(interfaces.vanet().is_none());
            assert!(interfaces.virtual_tap().is_some());
            assert!(interfaces.cloud().is_some());
            assert!(interfaces.validate("Server").is_ok());
        }
    }

    #[test]
    fn obu_validate_rejects_cloud() {
        #[cfg(feature = "test_helpers")]
        {
            let (tun1, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun2, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun3, _) = node_lib::test_helpers::util::mk_shim_pair();

            let interfaces = NodeInterfaces {
                vanet: Some(Arc::new(tun1)),
                virtual_tap: Some(Arc::new(tun2)),
                cloud: Some(Arc::new(tun3)),
            };

            assert!(interfaces.validate("Obu").is_err());
        }
    }

    #[test]
    fn server_validate_rejects_vanet() {
        #[cfg(feature = "test_helpers")]
        {
            let (tun1, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun2, _) = node_lib::test_helpers::util::mk_shim_pair();
            let (tun3, _) = node_lib::test_helpers::util::mk_shim_pair();

            let interfaces = NodeInterfaces {
                vanet: Some(Arc::new(tun1)),
                virtual_tap: Some(Arc::new(tun2)),
                cloud: Some(Arc::new(tun3)),
            };

            assert!(interfaces.validate("Server").is_err());
        }
    }
}
