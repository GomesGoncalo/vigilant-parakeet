use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::os::unix::io::{FromRawFd, IntoRawFd};
use tokio::runtime::Runtime;
use common::device::DeviceIo;
use nix::unistd::{pipe, close};

/// AsyncFd-driven bench that uses a pipe writer wrapped in `AsyncFd<DeviceIo>` and
/// a Tokio runtime to exercise the same async path `Device::send` would use.
fn bench_device_async_send(c: &mut Criterion) {
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

    // Wrap the writer fd in DeviceIo and AsyncFd
    let async_fd = tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(raw_w) }).expect("create asyncfd");

    // Construct a Device using the helper so we exercise Device::send path.
    let mac: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let device = common::device::Device::from_asyncfd_for_bench(mac, async_fd);

    // Create a single-threaded runtime used for bench iterations to avoid creating/dropping a runtime repeatedly.
    let rt = Runtime::new().expect("tokio rt");

    let buf = vec![0u8; 1024];

    c.bench_function("device_send_1k", |b| {
        b.iter(|| {
            // Call Device::send inside the runtime; this uses AsyncFd.writable internally.
            let res = rt.block_on(async { device.send(black_box(&buf[..])).await });
            match res {
                Ok(_n) => {}
                Err(e) => panic!("device send error: {}", e),
            }
        })
    });

    // cleanup the reader end. The writer fd will be closed when `device` is dropped.
    let _ = close(raw_r);
}

criterion_group!(benches, bench_device_async_send);
criterion_main!(benches);
