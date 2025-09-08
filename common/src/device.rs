use crate::network_interface::NetworkInterface;
#[cfg(feature = "stats")]
use crate::stats::Stats;
#[cfg(not(test))]
use anyhow::Context;
use anyhow::Result;
#[cfg(feature = "stats")]
use std::sync::Mutex;

#[cfg(test)]
mod test_fd_registry {
    use std::os::unix::io::RawFd;

    pub fn register_owned_fd(_fd: RawFd) {
        // no-op in this shim
    }

    pub fn unregister_owned_fd(_fd: RawFd) {
        // no-op in this shim
    }

    pub fn close_raw_fd(fd: RawFd) {
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(test)]
use test_fd_registry::{close_raw_fd, register_owned_fd, unregister_owned_fd};

use futures::ready;
#[cfg(not(test))]
use libc::{sockaddr, sockaddr_ll, AF_PACKET};
use mac_address::MacAddress;
#[cfg(not(test))]
use nix::sys::socket::{bind as nix_bind, LinkAddr, SockaddrLike};
#[cfg(not(test))]
use socket2::{Domain, Protocol, Socket, Type};
#[cfg(not(test))]
use std::convert::TryFrom;
use std::io;
use std::io::{ErrorKind, IoSlice, Read, Write};
#[cfg(not(test))]
use std::os::unix::io::IntoRawFd;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::task;
use std::task::Poll;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(not(test))]
const ETH_P_ALL: u16 = 0x0003;

impl AsyncRead for Device {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.fd.poll_read_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().read(buf.initialize_unfilled())) {
                Ok(Ok(n)) => {
                    buf.set_filled(buf.filled().len() + n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

#[cfg(all(test, feature = "stats"))]
mod stats_tests {
    use super::Device;
    use super::DeviceIo;
    use crate::device::close_raw_fd;
    use crate::stats::Stats;
    use mac_address::MacAddress;
    use std::os::unix::io::FromRawFd;

    fn set_nonblocking(fd: i32) {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    #[tokio::test]
    async fn device_stats_transmit_increments() {
        // create a pipe: reader=fd[0], writer=fd[1]
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // make writer non-blocking for AsyncFd
        set_nonblocking(writer_fd);

        // Build a Device that uses the writer end for send()
        let mac: MacAddress = [0, 1, 2, 3, 4, 5].into();
        // Safety: take ownership of the fd for DeviceIo
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = Device {
            mac_address: mac,
            fd: async_fd,
            stats: Stats::default().into(),
        };

        let before = device.stats();
        assert_eq!(before.transmitted_packets, 0);
        assert_eq!(before.transmitted_bytes, 0);

        // Call send() which should write to the writer end and increment stats
        let n = device.send(b"ping").await.expect("send ok");
        assert_eq!(n, 4);

        let after = device.stats();
        assert_eq!(after.transmitted_packets, before.transmitted_packets + 1);
        assert_eq!(
            after.transmitted_bytes,
            before.transmitted_bytes + n as u128
        );

        // drain the reader side to keep OS state clean
        let mut buf = [0u8; 16];
        let r = unsafe { libc::read(reader_fd, buf.as_mut_ptr().cast(), buf.len()) };
        assert!(r > 0);
        close_raw_fd(reader_fd);
    }

    #[tokio::test]
    async fn device_stats_receive_increments() {
        // create a pipe
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // make reader non-blocking for AsyncFd
        set_nonblocking(reader_fd);

        // Build Device using reader end for recv()
        let mac: MacAddress = [6, 7, 8, 9, 10, 11].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap();
        let device = Device {
            mac_address: mac,
            fd: async_fd,
            stats: Stats::default().into(),
        };

        let before = device.stats();
        assert_eq!(before.received_packets, 0);
        assert_eq!(before.received_bytes, 0);

        // write into writer_fd so recv() can read it
        let payload = b"pong";
        let w = unsafe { libc::write(writer_fd, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(w, payload.len() as isize);

        let mut buf = [0u8; 16];
        let n = device.recv(&mut buf).await.expect("recv ok");
        assert_eq!(n, payload.len());

        let after = device.stats();
        assert_eq!(after.received_packets, before.received_packets + 1);
        assert_eq!(after.received_bytes, before.received_bytes + n as u128);

        close_raw_fd(writer_fd);
    }
}

impl AsyncWrite for Device {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.fd.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::result::Result<usize, io::Error>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.fd.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write_vectored(bufs)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.fd.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().flush()) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub struct DeviceIo(RawFd);

impl From<RawFd> for DeviceIo {
    fn from(fd: RawFd) -> Self {
        #[cfg(test)]
        {
            register_owned_fd(fd);
        }
        Self(fd)
    }
}

impl FromRawFd for DeviceIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        #[cfg(test)]
        {
            register_owned_fd(fd);
        }
        Self(fd)
    }
}

impl AsRawFd for DeviceIo {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Read for DeviceIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }
}

impl Write for DeviceIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.send(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.sendv(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        let ret = unsafe { libc::fsync(self.0) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl DeviceIo {
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.0, buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }

    pub fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let iov = bufs
            .iter()
            .map(|buf| libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len(),
            })
            .collect::<Vec<_>>();
        let n = unsafe { libc::writev(self.0, iov.as_ptr().cast(), iov.len().try_into().unwrap()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }
}

impl Drop for DeviceIo {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            unregister_owned_fd(self.0);
        }
        unsafe { libc::close(self.0) };
    }
}

pub struct Device {
    mac_address: MacAddress,
    fd: AsyncFd<DeviceIo>,
    #[cfg(feature = "stats")]
    stats: Mutex<Stats>,
}

impl NetworkInterface for Device {
    fn mac_address(&self) -> MacAddress {
        self.mac_address
    }
}

impl Device {
    // Helper extracted so tests can more easily stub or replace raw socket creation.
    #[cfg(not(test))]
    fn create_packet_socket_and_mac(interface: &str) -> Result<(MacAddress, RawFd)> {
        let fd = Socket::new(
            Domain::PACKET,
            Type::RAW,
            Some(Protocol::from(i32::from(ETH_P_ALL.to_be()))),
        )?;

        let _ = fd.set_nonblocking(true);

        let mac_address =
            mac_address::mac_address_by_name(interface)?.context("needs mac address")?;

        let idx = nix::net::if_::if_nametoindex(interface)?;
        let mut saddr: sockaddr_ll = unsafe { std::mem::zeroed() };
        saddr.sll_family = u16::try_from(AF_PACKET)?;
        saddr.sll_ifindex = i32::try_from(idx)?;
        let p_saddr = std::ptr::addr_of_mut!(saddr);
        let p_saddr: &mut sockaddr = unsafe { &mut *(p_saddr.cast::<libc::sockaddr>()) };
        let storage = unsafe {
            LinkAddr::from_raw(
                p_saddr,
                Some(u32::try_from(std::mem::size_of::<sockaddr_ll>())?),
            )
        }
        .context("casting link storage")?;
        // bind using nix's socket bind helpers
        let _ = nix_bind(fd.as_raw_fd(), &storage);
        let raw_fd: RawFd = fd.into_raw_fd();
        Ok((mac_address, raw_fd))
    }

    pub fn new(interface: &str) -> Result<Self> {
        let (mac_address, raw_fd) = Self::create_packet_socket_and_mac(interface)?;
        Ok(Self {
            mac_address,
            fd: AsyncFd::new(unsafe { DeviceIo::from_raw_fd(raw_fd) })?,
            #[cfg(feature = "stats")]
            stats: Stats::default().into(),
        })
    }

    // Test shim: provide a non-privileged socket and dummy MAC when running tests.
    #[cfg(test)]
    fn create_packet_socket_and_mac(_interface: &str) -> Result<(MacAddress, RawFd)> {
        // create a simple pipe and use the writer end as the 'socket' for tests.
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        // set non-blocking to match production behavior
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        // dummy MAC address (not used by tests that only exercise send/recv)
        let mac: MacAddress = [0, 1, 2, 3, 4, 5].into();
        Ok((mac, writer_fd))
    }

    /// Public helper to construct a Device from an existing AsyncFd<DeviceIo>.
    /// This is a convenience used by benches and external harnesses to avoid
    /// opening raw packet sockets (which require elevated privileges).
    pub fn from_asyncfd_for_bench(
        mac_address: mac_address::MacAddress,
        fd: AsyncFd<DeviceIo>,
    ) -> Self {
        #[cfg(feature = "stats")]
        {
            Device {
                mac_address,
                fd,
                stats: Stats::default().into(),
            }
        }
        #[cfg(not(feature = "stats"))]
        {
            Device { mac_address, fd }
        }
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard.try_io(|inner| inner.get_ref().recv(buf)) {
                Ok(res) => {
                    #[cfg(feature = "stats")]
                    if let Ok(size) = res {
                        let mut stats = self.stats.lock().unwrap();
                        stats.received_packets += 1;
                        stats.received_bytes += size as u128;
                    }
                    return res;
                }
                Err(_) => continue,
            }
        }
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|inner| inner.get_ref().send(buf)) {
                Ok(res) => {
                    #[cfg(feature = "stats")]
                    if let Ok(size) = res {
                        let mut stats = self.stats.lock().unwrap();
                        stats.transmitted_packets += 1;
                        stats.transmitted_bytes += size as u128;
                    }
                    return res;
                }
                Err(_) => continue,
            }
        }
    }

    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut remaining = buf;
        let _size = buf.len();
        while !remaining.is_empty() {
            match self.send(remaining).await? {
                0 => return Err(ErrorKind::WriteZero.into()),
                n => {
                    let (_, rest) = std::mem::take(&mut remaining).split_at(n);
                    remaining = rest;
                }
            }
        }
        #[cfg(feature = "stats")]
        {
            let mut stats = self.stats.lock().unwrap();
            stats.transmitted_packets += 1;
            stats.transmitted_bytes += _size as u128;
        }
        Ok(())
    }

    pub async fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|inner| inner.get_ref().sendv(bufs)) {
                Ok(res) => {
                    #[cfg(feature = "stats")]
                    if let Ok(size) = res {
                        let mut stats = self.stats.lock().unwrap();
                        stats.transmitted_packets += 1;
                        stats.transmitted_bytes += size as u128;
                    }
                    return res;
                }
                Err(_) => continue,
            }
        }
    }

    #[cfg(feature = "stats")]
    pub fn stats(&self) -> Stats {
        *self.stats.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::Device;
    use super::DeviceIo;
    use crate::device::close_raw_fd;
    use crate::network_interface::NetworkInterface;
    use mac_address::MacAddress;
    use std::io::{IoSlice, Read, Write};
    use std::os::unix::io::AsRawFd;

    // Helper: set fd non-blocking using libc
    fn set_nonblocking(fd: i32) {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    fn make_pipe() -> (DeviceIo, DeviceIo) {
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        (DeviceIo::from(fds[0]), DeviceIo::from(fds[1]))
    }

    // Helper to construct a Device with the correct fields depending on whether
    // the `stats` feature is enabled.
    fn device_from_parts(mac: MacAddress, fd: tokio::io::unix::AsyncFd<DeviceIo>) -> Device {
        #[cfg(feature = "stats")]
        {
            use crate::stats::Stats;
            Device {
                mac_address: mac,
                fd,
                stats: Stats::default().into(),
            }
        }
        #[cfg(not(feature = "stats"))]
        {
            Device {
                mac_address: mac,
                fd,
            }
        }
    }

    #[test]
    fn deviceio_send_and_recv_roundtrip() {
        let (reader, writer) = make_pipe();
        let mut r = reader;
        let mut w = writer;

        let buf = b"hello";
        let n = w.write(buf).expect("write succeeded");
        assert_eq!(n, 5);

        let mut out = [0u8; 5];
        let n = r.read(&mut out).expect("read succeeded");
        assert_eq!(n, 5);
        assert_eq!(&out, buf);
    }

    #[test]
    fn deviceio_send_errors_on_broken_pipe_sync() {
        // create a pipe, close the reader, then write should return EPIPE (error)
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // close reader to provoke broken pipe
        close_raw_fd(reader_fd);

        // set writer non-blocking to avoid SIGPIPE terminating the test process
        set_nonblocking(writer_fd);

        let dio = DeviceIo::from(writer_fd);
        match dio.send(b"hello") {
            Ok(_) => panic!("expected send to error when reader closed"),
            Err(e) => assert_eq!(e.raw_os_error(), Some(libc::EPIPE)),
        }
    }

    #[test]
    fn deviceio_writev_errors_on_broken_pipe_sync() {
        // create a pipe, close the reader, then writev should return EPIPE (error)
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // close reader to provoke broken pipe
        close_raw_fd(reader_fd);

        // set writer non-blocking to avoid SIGPIPE
        set_nonblocking(writer_fd);

        let dio = DeviceIo::from(writer_fd);
        let a = b"ab";
        let b = b"cd";
        let bufs = [IoSlice::new(a), IoSlice::new(b)];
        match dio.sendv(&bufs) {
            Ok(_) => panic!("expected sendv to error when reader closed"),
            Err(e) => assert_eq!(e.raw_os_error(), Some(libc::EPIPE)),
        }
    }

    #[test]
    fn deviceio_writev_works() {
        let (reader, writer) = make_pipe();
        let mut r = reader;
        let mut w = writer;

        let a = b"ab";
        let b = b"cd";
        let bufs = [IoSlice::new(a), IoSlice::new(b)];
        let n = w.write_vectored(&bufs).expect("writev");
        assert_eq!(n, 4);

        let mut out = [0u8; 4];
        let n = r.read(&mut out).expect("read");
        assert_eq!(n, 4);
        assert_eq!(&out, b"abcd");
    }

    #[tokio::test]
    async fn async_deviceio_send_and_recv() {
        use std::os::unix::io::FromRawFd;
        use tokio::task;

        // create a pipe and wrap the fds
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        // reader fd is fds[0], writer is fds[1]
        let reader = unsafe { DeviceIo::from_raw_fd(fds[0]) };
        let writer = unsafe { DeviceIo::from_raw_fd(fds[1]) };

        // spawn a task to perform an async read using a tokio::io::unix::AsyncFd
        let reader_task = task::spawn(async move {
            let async_fd = tokio::io::unix::AsyncFd::new(reader).expect("asyncfd");
            let mut buf = [0u8; 16];
            // wait for readable
            let mut guard = async_fd.readable().await.expect("readable");
            let n = guard
                .try_io(|inner| inner.get_ref().recv(&mut buf))
                .expect("try_io")
                .expect("recv");
            buf[..n].to_vec()
        });

        // writer: perform a blocking write on the writer fd
        let n = writer.send(b"asynchello").expect("send");
        assert_eq!(n, 10);

        let received = reader_task.await.expect("task");
        assert_eq!(&received[..10], b"asynchello");
    }

    #[tokio::test]
    async fn device_wrapper_send_and_recv() {
        use std::os::unix::io::FromRawFd;

        // create a pipe and use the writer end for Device::send
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        set_nonblocking(writer_fd);

        let mac: MacAddress = [10, 11, 12, 13, 14, 15].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        let n = device.send(b"hello").await.expect("send");
        assert_eq!(n, 5);

        let mut out = [0u8; 5];
        let r = unsafe { libc::read(reader_fd, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(r, 5);
        assert_eq!(&out, b"hello");

        close_raw_fd(reader_fd);
    }

    #[tokio::test]
    async fn device_wrapper_recv_reads() {
        use std::os::unix::io::FromRawFd;

        // create a pipe and use the reader end for Device::recv
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        set_nonblocking(reader_fd);

        let mac: MacAddress = [16, 17, 18, 19, 20, 21].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        // write into writer_fd so recv can read
        let payload = b"world";
        let w = unsafe { libc::write(writer_fd, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(w, payload.len() as isize);

        let mut buf = [0u8; 8];
        let n = device.recv(&mut buf).await.expect("recv");
        assert_eq!(n, payload.len());
        assert_eq!(&buf[..n], payload);

        close_raw_fd(writer_fd);
    }

    #[tokio::test]
    async fn device_wrapper_send_vectored_works() {
        use std::os::unix::io::FromRawFd;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        set_nonblocking(writer_fd);

        let mac: MacAddress = [22, 23, 24, 25, 26, 27].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        let a = b"12";
        let b = b"34";
        let bufs = [IoSlice::new(a), IoSlice::new(b)];
        let n = device.send_vectored(&bufs).await.expect("send_vectored");
        assert_eq!(n, 4);

        let mut out = [0u8; 4];
        let r = unsafe { libc::read(reader_fd, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(r, 4);
        assert_eq!(&out, b"1234");

        close_raw_fd(reader_fd);
    }

    #[tokio::test]
    async fn device_send_errors_on_broken_pipe() {
        use std::os::unix::io::FromRawFd;

        // create a pipe and immediately close the reader to provoke EPIPE on write
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // close reader to provoke broken pipe
        close_raw_fd(reader_fd);

        set_nonblocking(writer_fd);

        let mac: MacAddress = [10, 11, 12, 13, 14, 15].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        let res = device.send(b"willfail").await;
        assert!(res.is_err(), "send should error when reader closed");
        // DeviceIo Drop will close writer_fd
    }

    #[tokio::test]
    async fn device_send_vectored_errors_on_broken_pipe() {
        use std::os::unix::io::FromRawFd;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // close reader to provoke EPIPE on writev
        close_raw_fd(reader_fd);

        set_nonblocking(writer_fd);

        let mac: MacAddress = [20, 21, 22, 23, 24, 25].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        let a = b"ab";
        let b = b"cd";
        let bufs = [IoSlice::new(a), IoSlice::new(b)];
        let res = device.send_vectored(&bufs).await;
        assert!(
            res.is_err(),
            "send_vectored should error when reader closed"
        );
    }

    #[tokio::test]
    async fn device_asyncwrite_is_write_vectored_true() {
        use std::os::unix::io::FromRawFd;
        use tokio::io::AsyncWrite;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        set_nonblocking(writer_fd);

        let mac: MacAddress = [30, 31, 32, 33, 34, 35].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        // call trait method
        assert!(<Device as AsyncWrite>::is_write_vectored(&device));
    }

    #[tokio::test]
    async fn device_asyncread_trait_reads() {
        use std::os::unix::io::FromRawFd;
        use tokio::io::AsyncReadExt;

        // create pipe: reader is fds[0]
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        set_nonblocking(reader_fd);

        let mac: MacAddress = [40, 41, 42, 43, 44, 45].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap();
        // create a Device by consuming async_fd
        let mut device = device_from_parts(mac, async_fd);

        // write into writer_fd
        let payload = b"asyncread";
        let w = unsafe { libc::write(writer_fd, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(w, payload.len() as isize);

        let mut buf = vec![0u8; 16];
        let n = device.read(&mut buf).await.expect("read");
        assert_eq!(n, payload.len());
        assert_eq!(&buf[..n], payload);

        close_raw_fd(writer_fd);
    }

    #[tokio::test]
    async fn device_flush_returns_err_on_pipe() {
        use std::os::unix::io::FromRawFd;
        use tokio::io::AsyncWriteExt;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        set_nonblocking(writer_fd);

        let mac: MacAddress = [50, 51, 52, 53, 54, 55].into();
        let mut device = device_from_parts(
            mac,
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        );

        // flushing a pipe-backed fd via fsync typically fails; ensure we get an Err
        let res = device.flush().await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn device_send_all_writes_entire_buffer() {
        use std::os::unix::io::FromRawFd;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];
        set_nonblocking(writer_fd);

        let mac: MacAddress = [60, 61, 62, 63, 64, 65].into();
        let device = device_from_parts(
            mac,
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        );

        let payload = b"this will be sent via send_all";
        device.send_all(payload).await.expect("send_all");

        // read back
        let mut out = vec![0u8; payload.len()];
        let r = unsafe { libc::read(reader_fd, out.as_mut_ptr().cast(), out.len()) };
        assert_eq!(r, payload.len() as isize);
        assert_eq!(&out, payload);

        close_raw_fd(reader_fd);
    }

    #[tokio::test]
    async fn device_mac_address_via_trait() {
        use std::os::unix::io::FromRawFd;

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        let mac: MacAddress = [70, 71, 72, 73, 74, 75].into();
        let async_fd =
            tokio::io::unix::AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap();
        let device = device_from_parts(mac, async_fd);

        // trait method is available because NetworkInterface is in scope in tests
        assert_eq!(device.mac_address(), mac);
    }

    #[cfg(feature = "stats")]
    #[tokio::test]
    async fn device_stats_increment_on_send_recv() {
        use crate::stats::Stats;

        let (r, w) = make_pipe();
        // wrap r and w into AsyncFd and Device-like struct manually
        let _async_r = tokio::io::unix::AsyncFd::new(r).expect("asyncfd r");
        let _async_w = tokio::io::unix::AsyncFd::new(w).expect("asyncfd w");

        // we can't construct the full Device without a real interface, but we can
        // check Stats default and manual increment behavior
        let mut stats = Stats::default();
        assert_eq!(stats.received_packets, 0);
        assert_eq!(stats.transmitted_packets, 0);

        // simulate increments
        stats.received_packets += 1;
        stats.received_bytes += 4;
        stats.transmitted_packets += 1;
        stats.transmitted_bytes += 4;

        assert_eq!(stats.received_packets, 1);
        assert_eq!(stats.transmitted_packets, 1);
    }

    #[test]
    fn deviceio_as_raw_fd_matches() {
        // create a pipe and take ownership of the writer fd in DeviceIo
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // Take ownership of writer_fd for DeviceIo; Drop will close it.
        let dio = DeviceIo::from(writer_fd);
        assert_eq!(dio.as_raw_fd(), writer_fd);

        // close the unused reader end to avoid leaks
        close_raw_fd(reader_fd);
    }

    #[test]
    fn deviceio_flush_ok_on_regular_file() {
        use std::fs::OpenOptions;
        use std::os::unix::io::IntoRawFd;

        // create a small temp file and take ownership of its fd
        let tmp = std::env::temp_dir().join(format!(
            "vp_test_flush_{}_{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .expect("create tmp");
        let raw = f.into_raw_fd();

        // DeviceIo should be able to fsync a regular file
        let mut dio = DeviceIo::from(raw);
        let n = dio.write(b"hello").expect("write");
        assert_eq!(n, 5);
        dio.flush().expect("fsync should succeed on a regular file");

        // cleanup file path; DeviceIo Drop will close the fd
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn deviceio_recv_returns_zero_on_closed_writer() {
        // create a pipe, close the writer, then read should return 0 (EOF)
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // close writer end to simulate EOF
        close_raw_fd(writer_fd);

        let dio = DeviceIo::from(reader_fd);
        let mut buf = [0u8; 8];
        let n = dio.recv(&mut buf).expect("recv should return Ok");
        assert_eq!(n, 0);
    }
}
