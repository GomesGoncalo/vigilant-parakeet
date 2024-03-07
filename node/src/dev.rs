use anyhow::{Context, Result};
use libc::{sockaddr, sockaddr_ll, AF_PACKET};
use mac_address::MacAddress;
use nix::sys::socket::{self, LinkAddr, SockaddrLike};
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use tokio::io::unix::AsyncFd;

const ETH_P_ALL: u16 = 0x0003;

pub struct Device {
    pub mac_address: MacAddress,
    fd: AsyncFd<DeviceIo>,
}

pub struct DeviceIo(RawFd);

impl From<RawFd> for DeviceIo {
    fn from(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl FromRawFd for DeviceIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
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
        let n = unsafe { libc::read(self.0, buf.as_ptr() as *mut _, buf.len() as _) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.0, buf.as_ptr() as *const _, buf.len() as _) };
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
                iov_len: buf.len() as _,
            })
            .collect::<Vec<_>>();
        let n = unsafe { libc::writev(self.0, iov.as_ptr() as *const _, iov.len() as _) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }
}

impl Drop for DeviceIo {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl Device {
    pub fn new(interface: &str) -> Result<Self> {
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
        let _ = socket::bind(fd.as_raw_fd(), &storage);
        let raw_fd: RawFd = fd.into_raw_fd();
        Ok(Self {
            mac_address,
            fd: AsyncFd::new(unsafe { DeviceIo::from_raw_fd(raw_fd) })?,
        })
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard.try_io(|inner| inner.get_ref().recv(buf)) {
                Ok(res) => {
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
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            match self.send(remaining).await? {
                0 => return Err(ErrorKind::WriteZero.into()),
                n => {
                    let (_, rest) = std::mem::take(&mut remaining).split_at(n);
                    remaining = rest;
                }
            }
        }
        Ok(())
    }

    pub async fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|inner| inner.get_ref().sendv(bufs)) {
                Ok(res) => {
                    return res;
                }
                Err(_) => continue,
            }
        }
    }
}
