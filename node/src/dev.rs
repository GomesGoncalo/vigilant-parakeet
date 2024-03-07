use anyhow::{bail, Context, Result};
use libc::{c_void, sockaddr, sockaddr_ll, AF_PACKET, MAP_SHARED};
use mac_address::MacAddress;
use nix::errno::Errno;
use nix::sys::socket::{self, LinkAddr, SockaddrLike};
use socket2::{Domain, Protocol, Socket, Type};
use std::ffi::{c_int, c_uint};
use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use tokio::io::unix::AsyncFd;

const ETH_P_ALL: u16 = 0x0003;
const PACKET_RX_RING: c_int = 5;
const PACKET_TX_RING: c_int = 13;
const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct TPacketReq {
    tp_block_size: c_uint,
    tp_block_nr: c_uint,
    tp_frame_size: c_uint,
    tp_frame_nr: c_uint,
}

#[derive(Debug)]
struct PacketRing {
    info: TPacketReq,
    start: *mut c_void,
    size: c_uint,
}

struct MappedMemory {
    pointer: *mut c_void,
    size: c_uint,
    tx_ring: PacketRing,
    rx_ring: PacketRing,
}

impl MappedMemory {
    pub fn new(fd: &impl AsRawFd) -> Result<Self> {
        let blocksiz: c_uint = 1 << 22;
        let framesiz: c_uint = 1 << 11;
        let blocknum: c_uint = 64;

        let mut req = TPacketReq {
            tp_block_size: blocksiz,
            tp_frame_size: framesiz,
            tp_block_nr: blocknum,
            tp_frame_nr: (blocksiz * blocknum) / framesiz,
        };

        tracing::info!(?req, "setting up rx ring");
        let p_req: *mut c_void = &mut req as *mut _ as *mut c_void;
        let res = unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_PACKET,
                PACKET_RX_RING,
                p_req,
                std::mem::size_of::<TPacketReq>().try_into().unwrap(),
            )
        };
        Errno::result(res)?;
        tracing::info!("done with {res}");
        tracing::info!(?req, "setting up tx ring");
        let res = unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_PACKET,
                PACKET_TX_RING,
                p_req,
                std::mem::size_of::<TPacketReq>().try_into().unwrap(),
            )
        };
        Errno::result(res)?;
        tracing::info!("done with {res}");

        let size: c_uint = req.tp_block_size * req.tp_block_nr;
        let size_d: c_uint = size * 2;
        tracing::info!("mapping {size_d}");
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_d.try_into().unwrap(),
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };

        if pointer as isize == -1 {
            bail!("could not create mapped memory");
        }

        tracing::info!("mapped @ {:p}", pointer);

        let tx_ring = PacketRing {
            info: req,
            start: pointer,
            size,
        };
        let rx_ring = PacketRing {
            info: req,
            start: unsafe { pointer.offset(size.try_into().unwrap()) },
            size,
        };

        Ok(Self {
            pointer,
            size: size_d,
            tx_ring,
            rx_ring,
        })
    }
}

impl Drop for MappedMemory {
    fn drop(&mut self) {
        let ret = unsafe { libc::munmap(self.pointer, self.size.try_into().unwrap()) };
        if ret == 0 {
            tracing::info!("unmapped {:p} size {}", self.pointer, self.size);
        } else {
            tracing::info!("failed to unmap {:p} size {}", self.pointer, self.size);
        }
    }
}

unsafe impl Send for MappedMemory {}
unsafe impl Sync for MappedMemory {}

pub struct Device {
    pub mac_address: MacAddress,
    fd: AsyncFd<DeviceIo>,
    memory: Option<MappedMemory>,
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
    pub fn new(interface: &str, memory_mapped: bool) -> Result<Self> {
        let fd = Socket::new(
            Domain::PACKET,
            Type::RAW,
            Some(Protocol::from(i32::from(ETH_P_ALL.to_be()))),
        )?;

        let _ = fd.set_nonblocking(true);

        let memory = if memory_mapped {
            Some(MappedMemory::new(&fd)?)
        } else {
            None
        };

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
            memory,
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
