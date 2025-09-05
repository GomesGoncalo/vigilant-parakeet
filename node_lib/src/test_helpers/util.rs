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

/// Mocked-time version of poll_until that uses tokio::time::advance instead of sleep.
/// This works with tokio::time::pause() for deterministic, fast tests.
/// Uses smaller time increments to allow Hub delay simulation to work correctly.
pub async fn poll_until_mocked<T, F>(mut check: F, attempts: u32, delay_ms: u64) -> Option<T>
where
    F: FnMut() -> Option<T>,
{
    for _ in 0..attempts {
        if let Some(v) = check() {
            return Some(v);
        }
        // Use smaller increments (10ms max) to allow Hub packet delivery to work correctly
        let increment = delay_ms.min(10);
        tokio::time::advance(Duration::from_millis(increment)).await;
    }
    None
}

/// Try to recv from a shim `Tun` with a per-attempt timeout. Returns the
/// number of bytes read on success, or `None` if no successful recv occurred
/// within the given attempts.
pub async fn poll_tun_recv_with_timeout(
    tun: &tun::Tun,
    buf: &mut [u8],
    timeout_ms: u64,
    attempts: u32,
) -> Option<usize> {
    for _ in 0..attempts {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), tun.recv(buf)).await {
            Ok(Ok(n)) => return Some(n),
            Ok(Err(_)) => continue,
            Err(_) => continue, // timed out
        }
    }
    None
}

/// Mocked-time version of poll_tun_recv_with_timeout that works with tokio::time::pause().
/// Instead of using timeouts, uses small time advances to allow data to arrive.
pub async fn poll_tun_recv_with_timeout_mocked(
    tun: &tun::Tun,
    buf: &mut [u8],
    timeout_ms: u64,
    attempts: u32,
) -> Option<usize> {
    for _ in 0..attempts {
        // Try to receive immediately (non-blocking)
        match tun.recv(buf).await {
            Ok(n) => return Some(n),
            Err(_) => {
                // No data available, advance time a bit and try again
                let advance_ms = (timeout_ms / 10).clamp(1, 10); // Small increments
                tokio::time::advance(Duration::from_millis(advance_ms)).await;
            }
        }
    }
    None
}

/// Poll until a specific expected payload is observed on `tun` within the
/// attempt/time budget. Uses an internal 256-byte buffer for reads.
pub async fn poll_tun_recv_expected(
    tun: &tun::Tun,
    expected: &[u8],
    timeout_ms: u64,
    attempts: u32,
) -> bool {
    let mut buf = vec![0u8; 256];
    for _ in 0..attempts {
        if let Some(n) = poll_tun_recv_with_timeout(tun, &mut buf, timeout_ms, 1).await {
            if n >= expected.len() && buf[..expected.len()] == expected[..] {
                return true;
            }
        }
    }
    false
}

/// Mocked-time version of poll_tun_recv_expected that works with tokio::time::pause().
pub async fn poll_tun_recv_expected_mocked(
    tun: &tun::Tun,
    expected: &[u8],
    timeout_ms: u64,
    attempts: u32,
) -> bool {
    let mut buf = vec![0u8; 256];
    for _ in 0..attempts {
        if let Some(n) =
            poll_tun_recv_with_timeout_mocked(tun, &mut buf, timeout_ms, attempts).await
        {
            if n >= expected.len() && buf[..expected.len()] == expected[..] {
                return true;
            }
        }
    }
    false
}

/// Repeat an async send function `times` times with `delay_ms` between attempts.
/// The send closure is expected to be an async function returning Result<(), E>.
pub async fn repeat_async_send<F, Fut, E>(mut send_fn: F, times: u32, delay_ms: u64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
{
    for _ in 0..times {
        let _ = send_fn().await;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}

/// Mocked-time version of repeat_async_send that uses tokio::time::advance instead of sleep.
/// This works with tokio::time::pause() for deterministic, fast tests.
pub async fn repeat_async_send_mocked<F, Fut, E>(mut send_fn: F, times: u32, delay_ms: u64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
{
    for _ in 0..times {
        let _ = send_fn().await;
        tokio::time::advance(Duration::from_millis(delay_ms)).await;
    }
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
use std::io;

/// Create `n` socketpairs and return (node_fds, hub_fds) or an io::Error.
pub fn mk_socketpairs(n: usize) -> io::Result<(Vec<i32>, Vec<i32>)> {
    let mut node_fds = Vec::with_capacity(n);
    let mut hub_fds = Vec::with_capacity(n);
    for _ in 0..n {
        let mut fds = [0; 2];
        unsafe {
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
            if r != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        node_fds.push(fds[0]);
        hub_fds.push(fds[1]);
    }
    Ok((node_fds, hub_fds))
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

/// Mocked-time version of mk_hub_with_checks that creates a Hub compatible with tokio::time::pause().
/// This Hub will properly deliver packets when tokio::time::advance() is called.
pub fn mk_hub_with_checks_mocked_time(
    hub_fds: Vec<i32>,
    delays_ms: Vec<Vec<u64>>,
    checks: Vec<std::sync::Arc<dyn crate::test_helpers::hub::HubCheck>>,
) {
    let hub =
        crate::test_helpers::hub::Hub::new_with_mocked_time(hub_fds, delays_ms).with_checks(checks);
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
    let expected_len = n
        .checked_mul(n)
        .ok_or_else(|| anyhow::anyhow!("overflow computing n*n for hub_fds length={}", n))?;
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

/// Safely shutdown the read side of a socket/file descriptor.
/// This wraps the unsafe libc call and documents intent at call sites.
pub fn shutdown_read(fd: i32) {
    // Intentionally ignore the return value; callers use this to provoke
    // EPIPE/EINVAL conditions on the peer without fully closing the fd.
    unsafe {
        let _ = libc::shutdown(fd, libc::SHUT_RD);
    }
}

/// Create a pipe and make the writer end non-blocking. Returns (reader_fd, writer_fd).
pub fn mk_pipe_nonblocking() -> std::io::Result<(i32, i32)> {
    let mut fds = [0; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) != 0 {
            // attempt to close fds before returning error
            let _ = libc::close(fds[0]);
            let _ = libc::close(fds[1]);
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok((fds[0], fds[1]))
}

/// Read up to buf.len() bytes from fd into buf. Returns number of bytes read or io::Error.
pub fn read_fd(fd: i32, buf: &mut [u8]) -> std::io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Close a file descriptor, returning an io::Result.
pub fn close_fd(fd: i32) -> std::io::Result<()> {
    let r = unsafe { libc::close(fd) };
    if r != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Helper function that implements timeout pattern with tokio::select!
/// Either awaits the future or times out after the specified duration.
/// Uses mocked time advancement for timeouts.
pub async fn await_with_timeout<T>(
    future: impl std::future::Future<Output = T>,
    timeout: Duration,
) -> Result<T, &'static str> {
    tokio::select! {
        result = future => Ok(result),
        _ = tokio::time::sleep(timeout) => Err("timeout"),
    }
}
