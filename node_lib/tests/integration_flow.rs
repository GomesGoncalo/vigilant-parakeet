use common::device::Device;
use common::device::DeviceIo;
use common::tun::test_tun::TokioTun;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::control::node::{handle_messages, ReplyType};
use std::os::unix::io::FromRawFd;
use std::sync::Arc;
use tokio::io::unix::AsyncFd;

#[tokio::test]
async fn handle_messages_sends_to_tun_and_device() {
    let (a, _b) = TokioTun::new_pair();
    let tun = Tun::from_shim_tun(a);

    // create a pipe to stand in for device fd
    let mut fds = [0; 2];
    unsafe { libc::pipe(fds.as_mut_ptr()) };
    let reader_fd = fds[0];
    let writer_fd = fds[1];

    // make writer non-blocking
    unsafe { libc::fcntl(writer_fd, libc::F_SETFL, libc::O_NONBLOCK) };

    let async_fd = AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
    let mac: MacAddress = [1u8; 6].into();
    let device = Device::from_asyncfd_for_bench(mac, async_fd);

    let tun = Arc::new(tun);
    let device = Arc::new(device);

    let tap = ReplyType::Tap(vec![vec![1u8, 2u8, 3u8]]);
    let wire = ReplyType::Wire(vec![vec![0u8; 14]]);

    let msgs = vec![tap, wire];
    handle_messages(msgs, &tun, &device, None)
        .await
        .expect("ok");

    // drain the reader side of the pipe to observe bytes written
    let mut buf = [0u8; 64];
    let n = unsafe { libc::read(reader_fd, buf.as_mut_ptr().cast(), buf.len()) };
    unsafe { libc::close(reader_fd) };
    unsafe { libc::close(writer_fd) };
    assert!(n > 0);
}
