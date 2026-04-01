//! Test helpers for OBU testing
//!
//! Provides factory functions for creating test ObuArgs with sensible defaults.

use crate::args::{ObuArgs, ObuParameters};

/// Create ObuArgs with minimal valid configuration for tests.
///
/// Default configuration:
/// - `hello_history: 2` (small for fast tests)
/// - `cached_candidates: 3`
/// - `enable_encryption: false`
/// - `mtu: 1500`
pub fn mk_test_obu_args() -> ObuArgs {
    ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: false,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 60_000,
            dh_key_lifetime_ms: 120_000,
            dh_reply_timeout_ms: 5_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with custom hello_history for tests.
pub fn mk_test_obu_args_with_history(hello_history: u32) -> ObuArgs {
    ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history,
            cached_candidates: 3,
            enable_encryption: false,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 60_000,
            dh_key_lifetime_ms: 120_000,
            dh_reply_timeout_ms: 5_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with encryption enabled for tests.
pub fn mk_test_obu_args_encrypted() -> ObuArgs {
    ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: true,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 60_000,
            dh_key_lifetime_ms: 120_000,
            dh_reply_timeout_ms: 5_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with encryption and DH enabled for tests.
pub fn mk_test_obu_args_dh() -> ObuArgs {
    ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: true,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 5_000,
            dh_key_lifetime_ms: 10_000,
            dh_reply_timeout_ms: 2_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with hello_history: 10 for integration tests.
/// This is an alias for compatibility with integration tests that need larger history.
pub fn mk_obu_args() -> ObuArgs {
    mk_test_obu_args_with_history(10)
}

/// Create ObuArgs with hello_history: 10 and encryption for integration tests.
pub fn mk_obu_args_encrypted() -> ObuArgs {
    ObuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: true,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 60_000,
            dh_key_lifetime_ms: 120_000,
            dh_reply_timeout_ms: 5_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with hello_history: 10, encryption, and DH for integration tests.
pub fn mk_obu_args_dh() -> ObuArgs {
    ObuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: true,
            enable_dh_signatures: false,
            dh_rekey_interval_ms: 1_000,
            dh_key_lifetime_ms: 10_000,
            dh_reply_timeout_ms: 2_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

/// Create ObuArgs with encryption and DH signatures enabled for integration tests.
pub fn mk_obu_args_dh_signed() -> ObuArgs {
    ObuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: true,
            enable_dh_signatures: true,
            dh_rekey_interval_ms: 1_000,
            dh_key_lifetime_ms: 10_000,
            dh_reply_timeout_ms: 2_000,
            cipher: node_lib::crypto::SymmetricCipher::default(),
            kdf: node_lib::crypto::KdfAlgorithm::default(),
            dh_group: node_lib::crypto::DhGroup::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mk_test_obu_args_defaults() {
        let args = mk_test_obu_args();
        assert_eq!(args.obu_params.hello_history, 2);
        assert_eq!(args.obu_params.cached_candidates, 3);
        assert!(!args.obu_params.enable_encryption);
        assert!(!args.obu_params.enable_dh_signatures);
    }

    #[test]
    fn test_mk_test_obu_args_with_history() {
        let args = mk_test_obu_args_with_history(10);
        assert_eq!(args.obu_params.hello_history, 10);
    }

    #[test]
    fn test_mk_test_obu_args_encrypted() {
        let args = mk_test_obu_args_encrypted();
        assert!(args.obu_params.enable_encryption);
    }

    #[test]
    fn test_mk_test_obu_args_dh() {
        let args = mk_test_obu_args_dh();
        assert!(args.obu_params.enable_encryption);
        assert_eq!(args.obu_params.dh_rekey_interval_ms, 5_000);
        assert_eq!(args.obu_params.dh_key_lifetime_ms, 10_000);
    }
}
