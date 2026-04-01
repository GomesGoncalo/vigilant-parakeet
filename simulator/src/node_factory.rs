use anyhow::Result;
use config::Config;
use mac_address::MacAddress;
use server_lib::Server;
use std::collections::HashMap;
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
            .with_mtu(1400) // Match OBU MTU (accounts for encryption + cloud protocol overhead)
            .build_tap()?;

        // Create cloud interface for infrastructure connection (RSU forwarding).
        // Use a /24 netmask so the kernel generates a subnet route, enabling ARP
        // resolution between nodes that share the 172.x.x.x cloud network.
        let cloud_tun = InterfaceBuilder::new("cloud")
            .with_ip(cloud_ip)
            .with_mtu(1500) // Standard MTU for infrastructure connectivity
            .with_netmask(std::net::Ipv4Addr::new(255, 255, 255, 0))
            .build_tap()?;

        let enable_encryption = settings.get_bool("enable_encryption").unwrap_or(false);
        let enable_dh_signatures_server =
            settings.get_bool("enable_dh_signatures").unwrap_or(false);
        let crypto_config = parse_crypto_config(settings);
        let key_ttl_ms = settings
            .get_int("key_ttl_ms")
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(86_400_000);

        // Parse PKI allowlist: dh_signing_allowlist = { "MAC" = "hex_pubkey" }
        let dh_signing_allowlist = parse_dh_signing_allowlist(settings);

        let mut server = Server::new(cloud_ip, port, node_name)
            .with_tun(virtual_tun.clone())
            .with_encryption(enable_encryption)
            .with_key_ttl_ms(key_ttl_ms)
            .with_crypto_config(crypto_config)
            .with_dh_signatures(enable_dh_signatures_server);
        if !dh_signing_allowlist.is_empty() {
            server = server.with_dh_signing_allowlist(dh_signing_allowlist);
        }
        let server = Arc::new(server);

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
        let interfaces = NodeInterfaces::server(
            virtual_tun.clone(),
            cloud_tun.clone(),
            Some(virtual_ip),
            Some(cloud_ip),
        );
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
    // MTU accounts for full overhead: encryption (28B) + cloud protocol (15B) +
    // Ethernet header (14B) + UDP/IP headers (28B) must fit in cloud MTU 1500.
    // Max = 1500 - 28 - 15 - 14 - 28 = 1415; use 1400 for safety margin.
    let mtu: i32 = 1400;
    let hello_history: u32 = settings.get_int("hello_history")?.try_into()?;

    // Create VANET interface (the wireless medium where control/data messages flow)
    let vanet_tun = InterfaceBuilder::new("vanet").build_tap()?;

    // Create Device bound to VANET interface
    let dev = Arc::new(Device::new(vanet_tun.name())?);

    // Create node instance and organize interfaces
    if node_type == node_lib::args::NodeType::Obu {
        let ip = Ipv4Addr::from_str(&settings.get_string("ip")?)?;
        let enable_encryption = settings.get_bool("enable_encryption").unwrap_or(false);
        let crypto_config = parse_crypto_config(settings);
        let dh_key_lifetime_ms = settings
            .get_int("dh_key_lifetime_ms")
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(86_400_000);
        let dh_rekey_interval_ms = settings
            .get_int("dh_rekey_interval_ms")
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(dh_key_lifetime_ms / 2);
        let dh_reply_timeout_ms = settings
            .get_int("dh_reply_timeout_ms")
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(5_000);

        // Create virtual interface (for decapsulated data traffic) — OBU only
        let virtual_tun = InterfaceBuilder::new("virtual")
            .with_ip(ip)
            .with_mtu(mtu as u16)
            .build_tap()?;

        // Build ObuArgs
        let enable_dh_signatures = settings.get_bool("enable_dh_signatures").unwrap_or(false);
        let signing_key_seed = settings.get_string("signing_key_seed").ok();

        let obu_args = obu_lib::ObuArgs {
            bind: vanet_tun.name().to_string(),
            tap_name: Some("virtual".to_string()),
            ip: Some(ip),
            mtu,
            obu_params: obu_lib::ObuParameters {
                hello_history,
                cached_candidates,
                enable_encryption,
                enable_dh_signatures,
                signing_key_seed,
                dh_rekey_interval_ms,
                dh_key_lifetime_ms,
                dh_reply_timeout_ms,
                cipher: crypto_config.cipher,
                kdf: crypto_config.kdf,
                dh_group: crypto_config.dh_group,
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
        // RSU node — no virtual TAP, no encryption config
        // RSU forwards traffic between OBUs (VANET) and Server (cloud/UDP)

        // Cloud interface for server connectivity
        let external_ip_str = settings.get_string("external_tap_ip").map_err(|_| {
            anyhow::anyhow!("RSU node requires external_tap_ip configuration for cloud interface")
        })?;
        let external_ip = Ipv4Addr::from_str(&external_ip_str)?;

        tracing::info!(
            external_ip = %external_ip,
            "Creating cloud interface for RSU server connectivity"
        );

        let cloud_tun = InterfaceBuilder::new("cloud")
            .with_ip(external_ip)
            .with_mtu(1500)
            .with_netmask(std::net::Ipv4Addr::new(255, 255, 255, 0))
            .build_tap()?;

        // Optional server connectivity — read from the RSU config file.
        let server_ip = settings
            .get_string("server_ip")
            .ok()
            .and_then(|s| Ipv4Addr::from_str(&s).ok());
        let server_port = settings
            .get_int("server_port")
            .ok()
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(8080);

        let rsu_args = rsu_lib::RsuArgs {
            bind: vanet_tun.name().to_string(),
            mtu,
            cloud_ip: Some(external_ip),
            rsu_params: rsu_lib::RsuParameters {
                hello_history,
                hello_periodicity: settings
                    .get_int("hello_periodicity")
                    .map(|x| u32::try_from(x).ok())
                    .ok()
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("hello_periodicity is required"))?,
                cached_candidates,
                server_ip,
                server_port,
            },
        };

        let node = SimNode::Rsu(rsu_lib::create_with_vdev(rsu_args, dev.clone(), node_name)?);

        let interfaces = NodeInterfaces::rsu(vanet_tun, cloud_tun, Some(external_ip));
        Ok(NodeCreationResult::new(dev, interfaces, node))
    }
}

/// Parse the dh_signing_allowlist table from settings.
///
/// Expected YAML format:
/// ```yaml
/// dh_signing_allowlist:
///   "AA:BB:CC:DD:EE:FF": "aabbcc...64hexchars"
/// ```
fn parse_dh_signing_allowlist(settings: &Config) -> HashMap<MacAddress, [u8; 32]> {
    let raw: HashMap<String, config::Value> = match settings.get_table("dh_signing_allowlist") {
        Ok(m) => m,
        Err(_) => return HashMap::new(),
    };
    let mut out = HashMap::new();
    for (mac_str, pubkey_val) in raw {
        let mac = match mac_str.parse::<MacAddress>() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(mac = %mac_str, error = %e, "Invalid MAC in dh_signing_allowlist, skipping");
                continue;
            }
        };
        let hex = match pubkey_val.into_string() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(mac = %mac_str, error = %e, "Invalid pubkey value in dh_signing_allowlist, skipping");
                continue;
            }
        };
        if hex.len() != 64 {
            tracing::warn!(mac = %mac_str, "dh_signing_allowlist pubkey must be 64 hex chars, skipping");
            continue;
        }
        let bytes: Option<Vec<u8>> = (0..32)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect();
        match bytes.and_then(|b| <[u8; 32]>::try_from(b).ok()) {
            Some(arr) => {
                out.insert(mac, arr);
            }
            None => {
                tracing::warn!(mac = %mac_str, "Failed to decode dh_signing_allowlist pubkey hex, skipping");
            }
        }
    }
    out
}

/// Parse crypto configuration from settings, falling back to defaults.
fn parse_crypto_config(settings: &Config) -> node_lib::crypto::CryptoConfig {
    let cipher = match settings.get_string("cipher") {
        Ok(raw) => match raw.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(
                    %raw,
                    %err,
                    "Invalid cipher in configuration, falling back to default"
                );
                Default::default()
            }
        },
        Err(_) => Default::default(),
    };

    let kdf = match settings.get_string("kdf") {
        Ok(raw) => match raw.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(
                    %raw,
                    %err,
                    "Invalid kdf in configuration, falling back to default"
                );
                Default::default()
            }
        },
        Err(_) => Default::default(),
    };

    let dh_group = match settings.get_string("dh_group") {
        Ok(raw) => match raw.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(
                    %raw,
                    %err,
                    "Invalid dh_group in configuration, falling back to default"
                );
                Default::default()
            }
        },
        Err(_) => Default::default(),
    };
    node_lib::crypto::CryptoConfig {
        cipher,
        kdf,
        dh_group,
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

        // Verify RSU has correct interfaces — RSU has no virtual TAP (server owns it)
        assert!(result.interfaces.vanet().is_some());
        assert!(result.interfaces.virtual_tap().is_none());
        assert!(result.interfaces.cloud().is_some());

        Ok(())
    }
}
