#[cfg(test)]
mod tests {
    use rsu_lib::*;

    #[test]
    fn rsu_lib_smoke() {
        let args = RsuArgs {
            bind: "lo".to_string(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 1,
                hello_periodicity: 1000,
                cached_candidates: 1,
                server_ip: None,
                server_port: 8080,
            },
        };
        let _ = args;
    }
}
