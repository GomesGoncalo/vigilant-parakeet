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
/// - `mtu: 1500`
pub fn mk_test_rsu_args() -> RsuArgs {
    mk_test_rsu_args_with_periodicity(5000)
}

/// Create RsuArgs with custom hello_periodicity for tests.
pub fn mk_test_rsu_args_with_periodicity(hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::new(),
        mtu: 1500,
        cloud_ip: None,
        rsu_params: RsuParameters {
            hello_history: 2,
            hello_periodicity,
            cached_candidates: 3,
            server_ip: None,
            server_port: 8080,
        },
    }
}

/// Create RsuArgs with custom hello_history for tests.
pub fn mk_test_rsu_args_with_history(hello_history: u32, hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::new(),
        mtu: 1500,
        cloud_ip: None,
        rsu_params: RsuParameters {
            hello_history,
            hello_periodicity,
            cached_candidates: 3,
            server_ip: None,
            server_port: 8080,
        },
    }
}

/// Create RsuArgs with hello_history: 10 for integration tests.
/// This is an alias for compatibility with integration tests that need larger history.
pub fn mk_rsu_args(hello_periodicity: u32) -> RsuArgs {
    mk_test_rsu_args_with_history(10, hello_periodicity)
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
    }

    #[test]
    fn test_mk_test_rsu_args_with_periodicity() {
        let args = mk_test_rsu_args_with_periodicity(3000);
        assert_eq!(args.rsu_params.hello_periodicity, 3000);
    }
}
