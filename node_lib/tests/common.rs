use obu_lib::{ObuArgs, ObuParameters};
use rsu_lib::{RsuArgs, RsuParameters};

/// Helper to create RsuArgs with sensible defaults for tests.
#[allow(dead_code)]
pub fn mk_rsu_args(hello_periodicity: u32) -> RsuArgs {
    RsuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 10,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: false,
        },
    }
}

/// Helper to create ObuArgs with sensible defaults for tests.
#[allow(dead_code)]
pub fn mk_obu_args() -> ObuArgs {
    ObuArgs {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: false,
        },
    }
}

/// Helper to create RsuArgs with encryption enabled.
#[allow(dead_code)]
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

/// Helper to create ObuArgs with encryption enabled.
#[allow(dead_code)]
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
        },
    }
}
