use common::device::DeviceIo;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nix::unistd::pipe;
use std::io::Read;
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;

/// AsyncFd-driven bench that uses a pipe writer wrapped in `AsyncFd<DeviceIo>` and
/// a Tokio runtime to exercise the same async path `Device::send` would use.
fn bench_device_async_send(_c: &mut Criterion) {
    // Create a pipe (reader, writer). nix 0.29 returns OwnedFd; consume them into RawFd.
    let (r_owned, w_owned) = pipe().expect("pipe");

    // Convert OwnedFd into raw fd integers to transfer ownership to DeviceIo.
    let raw_r = r_owned.into_raw_fd();
    let raw_w = w_owned.into_raw_fd();

    // Make the writer non-blocking so AsyncFd behaves correctly.
    unsafe {
        let flags = libc::fcntl(raw_w, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(raw_w, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    // Create a single-threaded runtime used for bench iterations to avoid creating/dropping a runtime repeatedly.
    let rt = Runtime::new().expect("tokio rt");
    // Enter the runtime context so AsyncFd::new can be called without panicking.
    let _enter = rt.enter();

    // Spawn a background thread that drains the reader end so the writer never stalls.
    let reader_handle = thread::spawn(move || {
        let mut f = unsafe { std::fs::File::from_raw_fd(raw_r) };
        let mut buf = [0u8; 4096];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => continue,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
            }
        }
    });

    // Wrap the writer fd in DeviceIo and AsyncFd
    let async_fd = tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(raw_w) })
        .expect("create asyncfd");

    // Construct a Device using the helper so we exercise Device::send path.
    let mac: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let device = common::device::Device::from_asyncfd_for_bench(mac, async_fd);

    let buf = vec![0u8; 1024];

    // Use a short, local Criterion config so running this in CI / interactively completes quickly
    // and still produces artifacts under target/criterion.
    let mut short_cfg = Criterion::default()
        .measurement_time(Duration::from_secs(1))
        .warm_up_time(Duration::from_secs(1))
        .sample_size(10);

    short_cfg.bench_function("device_send_1k", |b| {
        b.iter(|| {
            // Call Device::send inside the runtime; this uses AsyncFd.writable internally.
            let res = rt.block_on(async { device.send(black_box(&buf[..])).await });
            match res {
                Ok(_n) => {}
                Err(e) => panic!("device send error: {e}"),
            }
        })
    });

    // cleanup: drop the device (closes the writer) so the reader thread sees EOF, then join it.
    drop(device);
    let _ = reader_handle.join();
}

criterion_group!(device_send_recv_group, bench_device_async_send);
criterion_main!(device_send_recv_group);
