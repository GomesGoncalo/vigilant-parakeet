use anyhow::Result;
use common::tun::Tun;
use config::Config;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::Arc;

use common::device::Device;

use crate::simulator::SimNode;

/// Create a node (Device, virtual_tun, SimNode) from parsed settings and an existing node tun.
pub fn create_node_from_settings(
    node_type: node_lib::args::NodeType,
    settings: &Config,
    node_tun: Arc<Tun>,
) -> Result<(Arc<Device>, Arc<Tun>, SimNode)> {
    // Read optional cached_candidates; default to 3 when not present or invalid.
    let cached_candidates = settings
        .get_int("cached_candidates")
        .ok()
        .and_then(|x| u32::try_from(x).ok())
        .unwrap_or(3u32);

    // Common values used for both Obu and Rsu args
    let bind = node_tun.name().to_string();
    let tap_name = Some("virtual".to_string());
    let ip = Some(Ipv4Addr::from_str(&settings.get_string("ip")?)?);
    let mtu = 1436;
    let hello_history: u32 = settings.get_int("hello_history")?.try_into()?;
    let enable_encryption = settings.get_bool("enable_encryption").unwrap_or(false);

    #[cfg(not(feature = "test_helpers"))]
    let virtual_tun = Arc::new({
        let ip_addr = ip.ok_or_else(|| anyhow::anyhow!("IP address is required"))?;
        let real_tun = if let Some(ref name) = tap_name {
            tokio_tun::Tun::builder()
                .tap()
                .name(name)
                .address(ip_addr)
                .mtu(mtu)
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
        } else {
            tokio_tun::Tun::builder()
                .tap()
                .address(ip_addr)
                .mtu(mtu)
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
        };
        Tun::new_real(real_tun)
    });

    #[cfg(feature = "test_helpers")]
    let virtual_tun = {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        Arc::new(tun_a)
    };

    let dev = Arc::new(Device::new(node_tun.name())?);

    let vt = virtual_tun.clone();
    let node = if node_type == node_lib::args::NodeType::Obu {
        // Build ObuArgs directly
        let obu_args = obu_lib::ObuArgs {
            bind: bind.clone(),
            tap_name: tap_name.clone(),
            ip,
            mtu,
            obu_params: obu_lib::ObuParameters {
                hello_history,
                cached_candidates,
                enable_encryption,
            },
        };

        SimNode::Obu(obu_lib::create_with_vdev(
            obu_args,
            virtual_tun.clone(),
            dev.clone(),
        )?)
    } else {
        // Build RsuArgs directly
        let rsu_args = rsu_lib::RsuArgs {
            bind: bind.clone(),
            tap_name: tap_name.clone(),
            ip,
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

        SimNode::Rsu(rsu_lib::create_with_vdev(
            rsu_args,
            virtual_tun.clone(),
            dev.clone(),
        )?)
    };

    Ok((dev, vt, node))
}

#[cfg(all(test, feature = "test_helpers"))]
mod tests {
    use super::*;
    use anyhow::Result;
    use config::FileFormat;
    use std::sync::Arc;

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

        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let node_tun = Arc::new(tun_a);

        let (_dev, _vt, _node) =
            create_node_from_settings(node_lib::args::NodeType::Obu, &settings, node_tun)?;
        Ok(())
    }

    #[tokio::test]
    async fn create_node_rsu_from_settings() -> Result<()> {
        // build a minimal config for an RSU (requires hello_periodicity)
        let toml = r#"
            ip = '10.0.0.2'
            hello_history = 5
            hello_periodicity = 5000
        "#;
        let settings = Config::builder()
            .add_source(config::File::from_str(toml, FileFormat::Toml))
            .build()?;

        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let node_tun = Arc::new(tun_a);

        let (_dev, _vt, _node) =
            create_node_from_settings(node_lib::args::NodeType::Rsu, &settings, node_tun)?;
        Ok(())
    }
}
