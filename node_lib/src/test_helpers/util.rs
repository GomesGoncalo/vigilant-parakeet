use crate::args::{Args, NodeParameters, NodeType};
use common::device::{Device, DeviceIo};
use common::tun::{self, test_tun};
use mac_address::MacAddress;
use std::os::unix::io::FromRawFd;
use std::time::Duration;
use tokio::io::unix::AsyncFd;

/// Create NodeParameters with sensible defaults for tests.
pub fn mk_node_params(node_type: NodeType, hello_periodicity: Option<u32>) -> NodeParameters {
    NodeParameters {
        node_type,
        hello_history: 10,
        hello_periodicity,
        cached_candidates: 3,
        enable_encryption: false,
        server_address: None,
    }
}

/// Create a Device from a raw fd and mac address. Intended for tests only.
pub fn mk_device_from_fd(mac: MacAddress, fd: i32) -> Device {
    Device::from_asyncfd_for_bench(
        mac,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(fd) }).unwrap(),
    )
}

/// Create a test device with a fake file descriptor. Intended for tests only.
pub fn make_test_device(mac: MacAddress) -> Device {
    // Create a pipe to get valid file descriptors for testing
    let (reader_fd, _writer_fd) = mk_pipe_nonblocking()
        .expect("Failed to create pipe for test device");
    Device::from_asyncfd_for_bench(
        mac,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap(),
    )
}

/// Construct Args with sensible defaults used across integration tests.
pub fn mk_args(node_type: NodeType, hello_periodicity: Option<u32>) -> Args {
    Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(node_type, hello_periodicity),
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
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr());
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

/// Helper function for polling a condition with time advancement until it succeeds or times out.
/// This replaces the common pattern of manually advancing time in a loop.
///
/// # Arguments
/// * `step` - Duration to advance time on each iteration
/// * `check` - Closure that returns Some(T) when condition is met, None otherwise
/// * `timeout` - Maximum duration to wait before timing out
///
/// # Returns
/// * `Ok(T)` when condition is met
/// * `Err("timeout")` when timeout is reached
pub async fn await_condition_with_time_advance<T, F>(
    step: Duration,
    mut check: F,
    timeout: Duration,
) -> Result<T, &'static str>
where
    F: FnMut() -> Option<T>,
{
    await_with_timeout(
        async {
            loop {
                tokio::time::advance(step).await;
                if let Some(result) = check() {
                    return result;
                }
            }
        },
        timeout,
    )
    .await
}

/// Mocked-time version of advance_until that uses await_condition_with_time_advance internally.
/// Advances time by `step` duration until `check` returns true or `timeout` is reached.
/// Returns true if condition was met, false if timed out.
pub async fn advance_until<F>(mut check: F, step: Duration, timeout: Duration) -> bool
where
    F: FnMut() -> bool,
{
    await_condition_with_time_advance(
        step,
        || {
            if check() {
                Some(true)
            } else {
                None
            }
        },
        timeout,
    )
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn poll_until_immediate_and_none() {
        // immediate success
        let got = poll_until(|| Some(42u32), 3, 1).await;
        assert_eq!(got, Some(42));

        // always none
        let none: Option<u32> = poll_until(|| None, 2, 1).await;
        assert_eq!(none, None);
    }

    #[tokio::test]
    async fn poll_until_mocked_sees_change() {
        tokio::time::pause();

        let flag = Arc::new(AtomicU32::new(0));
        let f = flag.clone();

        // schedule a change after 15ms
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(15)).await;
            f.store(1, Ordering::SeqCst);
        });

        let res = poll_until_mocked(
            || {
                if flag.load(Ordering::SeqCst) == 1 {
                    Some(7u32)
                } else {
                    None
                }
            },
            10,
            10,
        )
        .await;

        assert_eq!(res, Some(7));
    }

    #[tokio::test]
    async fn repeat_async_send_mocked_increments() {
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        repeat_async_send_mocked(
            || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), ()>(())
                }
            },
            5,
            10,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn mk_pipe_nonblocking_and_read_write() {
        let (r, w) = mk_pipe_nonblocking().expect("create pipe");
        let payload = b"hi";
        let nw = unsafe { libc::write(w, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(nw, payload.len() as isize);

        let mut buf = [0u8; 2];
        let nr = read_fd(r, &mut buf).expect("read");
        assert_eq!(nr, 2);
        assert_eq!(&buf, payload);

        close_fd(r).expect("close r");
        close_fd(w).expect("close w");
    }

    #[test]
    fn mk_socketpairs_roundtrip_and_close() {
        let (node_fds, hub_fds) = mk_socketpairs(1).expect("socketpairs");
        let node = node_fds[0];
        let hub = hub_fds[0];

        let msg = b"yo";
        let n = unsafe { libc::send(node, msg.as_ptr().cast(), msg.len(), 0) };
        assert!(n >= 0, "send failed");

        let mut buf = [0u8; 8];
        let got = read_fd(hub, &mut buf).expect("read from hub");
        assert_eq!(got, msg.len());

        close_fd(node).expect("close node");
        close_fd(hub).expect("close hub");
    }

    #[tokio::test]
    async fn await_with_timeout_success_and_timeout() {
        // immediate future should succeed
        let ok = await_with_timeout(async { 9u32 }, Duration::from_millis(10)).await;
        assert_eq!(ok.unwrap(), 9);

        // now test timeout path using a pending oneshot receiver
        tokio::time::pause();
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        let fut = async {
            rx.await.unwrap();
            5u32
        };
        let pending = await_with_timeout(fut, Duration::from_millis(10));

        // advance time to trigger timeout
        tokio::time::advance(Duration::from_millis(20)).await;
        let res = pending.await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn advance_until_detects_flag() {
        tokio::time::pause();
        let flag = Arc::new(AtomicU32::new(0));
        let f = flag.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            f.store(1, Ordering::SeqCst);
        });

        let ok = advance_until(
            || flag.load(Ordering::SeqCst) == 1,
            Duration::from_millis(10),
            Duration::from_millis(200),
        )
        .await;
        assert!(ok);
    }
}
