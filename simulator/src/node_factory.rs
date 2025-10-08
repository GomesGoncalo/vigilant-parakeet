use anyhow::Result;
use config::Config;
use server_lib::Server;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use common::device::Device;

use crate::interface_builder::InterfaceBuilder;
use crate::node_interfaces::{NodeCreationResult, NodeInterfaces};
use crate::simulator::SimNode;

/// Create a node with all its interfaces from parsed settings.
///
/// Creates all necessary network interfaces within the namespace:
/// - **OBU**: vanet (VANET medium), virtual (decapsulated traffic)
/// - **RSU**: vanet (VANET medium), virtual (decapsulated traffic), cloud (infrastructure)
/// - **Server**: virtual (distributed network), cloud (infrastructure)
///
/// All interfaces are created inside this function with consistent naming.
/// Returns a `NodeCreationResult` containing the device, organized interfaces, and node instance.
pub fn create_node_from_settings(
    node_type: node_lib::args::NodeType,
    settings: &Config,
    node_name: String,
) -> Result<NodeCreationResult> {
    // Handle Server nodes separately - they receive UDP traffic from RSUs via infrastructure
    // Server nodes have TWO interfaces:
    // 1. "virtual" TAP interface: Communicates with OBU virtual devices through the distributed routing network (10.x.x.x)
    // 2. "cloud" interface: Infrastructure connection where RSUs forward encapsulated traffic (172.x.x.x)
    // Servers do NOT have a "real" interface - they don't participate in the VANET medium
    if node_type == node_lib::args::NodeType::Server {
        let virtual_ip = Ipv4Addr::from_str(&settings.get_string("virtual_ip")?)?;
        let cloud_ip = Ipv4Addr::from_str(&settings.get_string("cloud_ip")?)?;
        let port = settings
            .get_int("port")
            .ok()
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(8080);

        tracing::info!(
            virtual_ip = %virtual_ip,
            cloud_ip = %cloud_ip,
            port = port,
            "Creating Server node (UDP receiver)"
        );

        // Create virtual TAP interface for distributed network communication with OBUs
        let virtual_tun = InterfaceBuilder::new("virtual")
            .with_ip(virtual_ip)
            .with_mtu(1436) // Match OBU/RSU MTU
            .build_tap()?;

        // Create cloud interface for infrastructure connection (RSU forwarding)
        let cloud_tun = InterfaceBuilder::new("cloud")
            .with_ip(cloud_ip)
            .with_mtu(1500) // Standard MTU for infrastructure connectivity
            .build_tap()?;

        let server = Arc::new(Server::new(cloud_ip, port, node_name));

        // Start the server immediately in the namespace context using block_in_place
        // This ensures the socket binds within the correct network namespace
        // block_in_place allows running async code in a sync context without blocking the executor
        let server_clone = server.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Err(e) = server_clone.start().await {
                    tracing::error!(error = %e, "Failed to start server in namespace");
                    return Err(anyhow::anyhow!("Failed to start server: {}", e));
                }
                Ok(())
            })
        })?;

        // Create a dummy device for compatibility (servers don't use the Device abstraction)
        #[cfg(not(feature = "test_helpers"))]
        let dummy_device = Arc::new(Device::new(virtual_tun.name())?);
        #[cfg(feature = "test_helpers")]
        let dummy_device = {
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Arc::new(Device::new(tun_a.name())?)
        };

        // Organize interfaces and return result
    let interfaces = NodeInterfaces::server(virtual_tun.clone(), cloud_tun.clone(), Some(virtual_ip), Some(cloud_ip));
        return Ok(NodeCreationResult::new(
            dummy_device,
            interfaces,
            SimNode::Server(server),
        ));
    }

    // Read optional cached_candidates; default to 3 when not present or invalid.
    let cached_candidates = settings
        .get_int("cached_candidates")
        .ok()
        .and_then(|x| u32::try_from(x).ok())
        .unwrap_or(3u32);

    // Common values used for both Obu and Rsu args
    let ip = Ipv4Addr::from_str(&settings.get_string("ip")?)?;
    let mtu: i32 = 1436;
    let hello_history: u32 = settings.get_int("hello_history")?.try_into()?;
    let enable_encryption = settings.get_bool("enable_encryption").unwrap_or(false);

    // Create VANET interface (the wireless medium where control/data messages flow)
    let vanet_tun = InterfaceBuilder::new("vanet").build_tap()?;

    // Create virtual interface (for decapsulated data traffic)
    let virtual_tun = InterfaceBuilder::new("virtual")
    .with_ip(ip)
    .with_mtu(mtu as u16)
    .build_tap()?;

    // Create Device bound to VANET interface
    let dev = Arc::new(Device::new(vanet_tun.name())?);

    // For RSU nodes, create cloud interface for server connectivity
    // RSUs forward encapsulated traffic to servers via this interface
    let cloud_tun_opt = if node_type == node_lib::args::NodeType::Rsu {
        // Check if external_tap_ip is configured
        if let Ok(external_ip_str) = settings.get_string("external_tap_ip") {
            let external_ip = Ipv4Addr::from_str(&external_ip_str)?;

            tracing::info!(
                external_ip = %external_ip,
                "Creating cloud interface for RSU server connectivity"
            );

            let cloud_tun = InterfaceBuilder::new("cloud")
                .with_ip(external_ip)
                .with_mtu(1500) // Standard MTU for infrastructure connectivity
                .build_tap()?;

            Some(cloud_tun)
        } else {
            None
        }
    } else {
        None
    };

    // Create node instance and organize interfaces
    if node_type == node_lib::args::NodeType::Obu {
        // Build ObuArgs
        let obu_args = obu_lib::ObuArgs {
            bind: vanet_tun.name().to_string(),
            tap_name: Some("virtual".to_string()),
            ip: Some(ip),
            mtu,
            obu_params: obu_lib::ObuParameters {
                hello_history,
                cached_candidates,
                enable_encryption,
            },
        };

        let node = SimNode::Obu(obu_lib::create_with_vdev(
            obu_args,
            virtual_tun.clone(),
            dev.clone(),
            node_name,
        )?);

    let interfaces = NodeInterfaces::obu(vanet_tun, virtual_tun.clone(), Some(ip));
        Ok(NodeCreationResult::new(dev, interfaces, node))
    } else {
        // RSU node
        let cloud_tun = cloud_tun_opt.ok_or_else(|| {
            anyhow::anyhow!("RSU node requires external_tap_ip configuration for cloud interface")
        })?;

        let rsu_args = rsu_lib::RsuArgs {
            bind: vanet_tun.name().to_string(),
            tap_name: Some("virtual".to_string()),
            ip: Some(ip),
            mtu,
            rsu_params: rsu_lib::RsuParameters {
                hello_history,
                hello_periodicity: settings
                    .get_int("hello_periodicity")
                    .map(|x| u32::try_from(x).ok())
                    .ok()
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("hello_periodicity is required"))?,
                cached_candidates,
                enable_encryption,
            },
        };

        let node = SimNode::Rsu(rsu_lib::create_with_vdev(
            rsu_args,
            virtual_tun.clone(),
            dev.clone(),
            node_name,
        )?);

    // external_ip was created when building cloud_tun_opt; reuse it via parsing from cloud_tun interface isn't possible
    // Instead we have `external_ip` in the branch where cloud_tun was created; to keep types simple, pass Some(ip) for virtual and Some(external_ip) for cloud
    let interfaces = NodeInterfaces::rsu(vanet_tun, virtual_tun.clone(), cloud_tun, Some(ip), Some(ip));
        Ok(NodeCreationResult::new(dev, interfaces, node))
    }
}

#[cfg(all(test, feature = "test_helpers"))]
mod tests {
    use super::*;
    use anyhow::Result;
    use config::FileFormat;

    #[tokio::test]
    async fn create_node_obu_from_settings() -> Result<()> {
        // build a minimal config for an OBU
        let toml = r#"
            ip = '10.0.0.1'
            hello_history = 10
        "#;
        let settings = Config::builder()
            .add_source(config::File::from_str(toml, FileFormat::Toml))
            .build()?;

        let result = create_node_from_settings(
            node_lib::args::NodeType::Obu,
            &settings,
            "test_obu".to_string(),
        )?;

        // Verify OBU has correct interfaces
        assert!(result.interfaces.vanet().is_some());
        assert!(result.interfaces.virtual_tap().is_some());
        assert!(result.interfaces.cloud().is_none());

        Ok(())
    }

    #[tokio::test]
    async fn create_node_rsu_from_settings() -> Result<()> {
        // build a minimal config for an RSU (requires hello_periodicity and external_tap_ip)
        let toml = r#"
            ip = '10.0.0.2'
            hello_history = 5
            hello_periodicity = 5000
            external_tap_ip = '172.16.0.1'
        "#;
        let settings = Config::builder()
            .add_source(config::File::from_str(toml, FileFormat::Toml))
            .build()?;

        let result = create_node_from_settings(
            node_lib::args::NodeType::Rsu,
            &settings,
            "test_rsu".to_string(),
        )?;

        // Verify RSU has correct interfaces
        assert!(result.interfaces.vanet().is_some());
        assert!(result.interfaces.virtual_tap().is_some());
        assert!(result.interfaces.cloud().is_some());

        Ok(())
    }
}
