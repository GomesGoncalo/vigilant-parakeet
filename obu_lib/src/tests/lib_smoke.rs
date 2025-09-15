#[cfg(test)]
mod tests {
    use obu_lib::*;

    #[test]
    fn obu_lib_smoke() {
        // Ensure the crate exports ObuArgs and ObuParameters and that they can be constructed.
        let args = ObuArgs {
            bind: "lo".to_string(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 1,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let _ = args;
    }
}
