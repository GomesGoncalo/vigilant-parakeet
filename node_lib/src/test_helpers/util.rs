use crate::args::{Args, NodeParameters, NodeType};
use common::device::{Device, DeviceIo};
use common::tun::{self, test_tun};
use mac_address::MacAddress;
use std::os::unix::io::FromRawFd;
use std::time::Duration;
use tokio::io::unix::AsyncFd;

/// Create a Device from a raw fd and mac address. Intended for tests only.
pub fn mk_device_from_fd(mac: MacAddress, fd: i32) -> Device {
    Device::from_asyncfd_for_bench(
        mac,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(fd) }).unwrap(),
    )
}

/// Construct Args with sensible defaults used across integration tests.
pub fn mk_args(node_type: NodeType, hello_periodicity: Option<u32>) -> Args {
    Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type,
            hello_history: 10,
            hello_periodicity,
            cached_candidates: 3,
        },
    }
}

/// Async poll helper used in tests to wait for a condition with a delay.
pub async fn poll_until<T, F>(mut check: F, attempts: u32, delay_ms: u64) -> Option<T>
where
    F: FnMut() -> Option<T>,
{
    for _ in 0..attempts {
        if let Some(v) = check() {
            return Some(v);
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    None
}

/// Create a pair of shim TUNs and return them wrapped as `common::tun::Tun`.
pub fn mk_shim_pair() -> (tun::Tun, tun::Tun) {
    let (a, b) = test_tun::TokioTun::new_pair();
    (tun::Tun::new_shim(a), tun::Tun::new_shim(b))
}

/// Create `n` shim TUN pairs and return the pairs as a Vec of (Tun, Tun).
/// Each pair is a connected test shim (useful when you need the peer side).
pub fn mk_shim_pairs(n: usize) -> Vec<(tun::Tun, tun::Tun)> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let (a, b) = test_tun::TokioTun::new_pair();
        v.push((tun::Tun::new_shim(a), tun::Tun::new_shim(b)));
    }
    v
}

/// Create `n` shim TUNs and return them as a Vec<Tun>.
/// This discards the peer of each pair and returns the first endpoint only.
pub fn mk_shim_tuns(n: usize) -> Vec<tun::Tun> {
    mk_shim_pairs(n).into_iter().map(|(a, _)| a).collect()
}

/// Create `n` socketpairs and return (node_fds, hub_fds). Each entry is a raw fd.
pub fn mk_socketpairs(n: usize) -> (Vec<i32>, Vec<i32>) {
    let mut node_fds = Vec::with_capacity(n);
    let mut hub_fds = Vec::with_capacity(n);
    for _ in 0..n {
        let mut fds = [0; 2];
        unsafe {
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
            assert_eq!(r, 0, "socketpair failed");
            let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            let _ = libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
        }
        node_fds.push(fds[0]);
        hub_fds.push(fds[1]);
    }
    (node_fds, hub_fds)
}

/// Convenience wrapper: spawn a `Hub` with given hub_fds, delays, and checks.
///
/// - `hub_fds` -- vector of hub endpoint fds (one per node)
/// - `delays_ms` -- square matrix of per-link delays in milliseconds (size must match hub_fds.len())
/// - `checks` -- vector of Arc<dyn HubCheck> to be invoked for observed packets
pub fn mk_hub_with_checks(
    hub_fds: Vec<i32>,
    delays_ms: Vec<Vec<u64>>,
    checks: Vec<std::sync::Arc<dyn crate::test_helpers::hub::HubCheck>>,
) {
    let hub = crate::test_helpers::hub::Hub::new(hub_fds, delays_ms).with_checks(checks);
    hub.spawn();
}

/// Helper that accepts a flat delays vector of length N*N and constructs an N x N
/// delay matrix before spawning the Hub with checks.
///
/// Panics if `delays_flat.len() != hub_fds.len() * hub_fds.len()`.
use anyhow::Result;

pub fn mk_hub_with_checks_flat(
    hub_fds: Vec<i32>,
    delays_flat: Vec<u64>,
    checks: Vec<std::sync::Arc<dyn crate::test_helpers::hub::HubCheck>>,
) -> Result<()> {
    let n = hub_fds.len();
    let expected_len = n.checked_mul(n).ok_or_else(|| {
        anyhow::anyhow!("overflow computing n*n for hub_fds length={}", n)
    })?;
    if delays_flat.len() != expected_len {
        return Err(anyhow::anyhow!(
            "delays_flat length must be n*n (got {} expected {})",
            delays_flat.len(),
            expected_len
        ));
    }
    let mut delays_ms: Vec<Vec<u64>> = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * n;
        let end = start + n;
        delays_ms.push(delays_flat[start..end].to_vec());
    }

    mk_hub_with_checks(hub_fds, delays_ms, checks);
    Ok(())
}
