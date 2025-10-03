//! Test helpers for RSU testing
//!
//! Provides factory functions for creating test RsuArgs with sensible defaults.

use crate::args::{RsuArgs, RsuParameters};

/// Create RsuArgs with minimal valid configuration for tests.
///
/// Default configuration:
/// - `hello_history: 2` (small for fast tests)
/// - `hello_periodicity: 5000` ms
/// - `cached_candidates: 3`
/// - `enable_encryption: false`
/// - `mtu: 1500`
pub fn mk_test_rsu_args() -> RsuArgs {
    mk_test_rsu_args_with_periodicity(5000)
}

/// Create RsuArgs with custom hello_periodicity for tests.
pub fn mk_test_rsu_args_with_periodicity(hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 2,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: false,
        },
    }
}

/// Create RsuArgs with custom hello_history for tests.
pub fn mk_test_rsu_args_with_history(hello_history: u32, hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: false,
        },
    }
}

/// Create RsuArgs with encryption enabled for tests.
pub fn mk_test_rsu_args_encrypted(hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 2,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: true,
        },
    }
}

/// Create RsuArgs with hello_history: 10 for integration tests.
/// This is an alias for compatibility with integration tests that need larger history.
pub fn mk_rsu_args(hello_periodicity: u32) -> RsuArgs {
    mk_test_rsu_args_with_history(10, hello_periodicity)
}

/// Create RsuArgs with hello_history: 10 and encryption for integration tests.
pub fn mk_rsu_args_encrypted(hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 10,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mk_test_rsu_args_defaults() {
        let args = mk_test_rsu_args();
        assert_eq!(args.rsu_params.hello_history, 2);
        assert_eq!(args.rsu_params.hello_periodicity, 5000);
        assert_eq!(args.rsu_params.cached_candidates, 3);
        assert!(!args.rsu_params.enable_encryption);
    }

    #[test]
    fn test_mk_test_rsu_args_with_periodicity() {
        let args = mk_test_rsu_args_with_periodicity(3000);
        assert_eq!(args.rsu_params.hello_periodicity, 3000);
    }

    #[test]
    fn test_mk_test_rsu_args_encrypted() {
        let args = mk_test_rsu_args_encrypted(4000);
        assert!(args.rsu_params.enable_encryption);
    }
}
