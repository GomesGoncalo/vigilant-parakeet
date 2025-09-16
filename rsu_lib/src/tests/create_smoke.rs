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

        let args = rsu_lib::RsuArgs {
            bind: "lo".into(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let node = rsu_lib::create_with_vdev(args, tun, dev)?;
        let _ = node.as_any();
        Ok(())
    }
}
