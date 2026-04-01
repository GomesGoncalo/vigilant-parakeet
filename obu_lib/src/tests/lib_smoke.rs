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
                enable_dh_signatures: false,
                signing_key_seed: None,
                dh_rekey_interval_ms: 60_000,
                dh_key_lifetime_ms: 120_000,
                dh_reply_timeout_ms: 5_000,
                cipher: node_lib::crypto::SymmetricCipher::default(),
                kdf: node_lib::crypto::KdfAlgorithm::default(),
                dh_group: node_lib::crypto::DhGroup::default(),
            },
        };
        let _ = args;
    }
}
