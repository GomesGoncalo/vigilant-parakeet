#[cfg(test)]
mod create_smoke {
    use anyhow::Result;
    use common::device::Device;
    use std::sync::Arc;

    #[tokio::test]
    async fn create_with_vdev_constructs_node() -> Result<()> {
        let dev = Arc::new(Device::new("lo")?);

        let args = rsu_lib::RsuArgs {
            bind: "lo".into(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000,
                cached_candidates: 1,
                server_ip: None,
                server_port: 8080,
            },
        };
        let node = rsu_lib::create_with_vdev(args, dev, "test_rsu".to_string())?;
        let _ = node.as_any();
        Ok(())
    }
}
