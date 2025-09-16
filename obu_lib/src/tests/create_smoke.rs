#[cfg(test)]
mod create_smoke {
    use anyhow::Result;
    use common::device::Device;
    use common::tun::Tun;
    use std::sync::Arc;

    #[tokio::test]
    async fn create_with_vdev_constructs_node() -> Result<()> {
        let (tun_a, _tun_b) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let dev = Arc::new(Device::new("lo")?);

        let args = obu_lib::ObuArgs {
            bind: "lo".into(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: obu_lib::ObuParameters {
                hello_history: 2,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let node = obu_lib::create_with_vdev(args, tun, dev)?;
        // Downcast check
        let _ = node.as_any();
        Ok(())
    }
}
