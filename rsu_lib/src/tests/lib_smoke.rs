#[cfg(test)]
mod tests {
    use rsu_lib::*;

    #[test]
    fn rsu_lib_smoke() {
        let args = RsuArgs {
            bind: "lo".to_string(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: RsuParameters {
                hello_history: 1,
                hello_periodicity: 1000,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let _ = args;
    }
}
