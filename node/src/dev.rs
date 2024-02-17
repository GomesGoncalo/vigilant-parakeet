use std::{io::IoSlice, os::fd::AsRawFd, sync::Arc};

use anyhow::{Context, Error, Result};
use libc::{sockaddr, sockaddr_ll, AF_PACKET};
use mac_address::MacAddress;
use nix::sys::socket::{self, LinkAddr, SockaddrLike};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{mpsc::Receiver, mpsc::Sender};
use uninit::uninit_array;

const ETH_P_ALL: u16 = 0x0003;

pub struct Device {
    pub mac_address: MacAddress,
    pub tx: Sender<OutgoingMessage>,
    socket: Arc<Socket>,
}

pub enum OutgoingMessage {
    Simple(Arc<[u8]>),
    Vectored(Vec<Arc<[u8]>>),
}

impl Device {
    pub fn new(interface: &str) -> Result<Self> {
        let iface = Arc::new(Socket::new(
            Domain::PACKET,
            Type::RAW,
            Some(Protocol::from(i32::from(ETH_P_ALL.to_be()))),
        )?);

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
        let _ = socket::bind(iface.as_raw_fd(), &storage);
        let (tx, mut inner_receiver): (Sender<_>, tokio::sync::mpsc::Receiver<_>) =
            tokio::sync::mpsc::channel(1024);

        let socket = iface.clone();
        tokio::spawn(async move {
            while let Some(i) = inner_receiver.recv().await {
                match match i {
                    OutgoingMessage::Simple(msg) => socket.send(&msg),
                    OutgoingMessage::Vectored(vec_msg) => {
                        let vec: Vec<IoSlice> = vec_msg.iter().map(|x| IoSlice::new(x)).collect();
                        socket.send_vectored(&vec)
                    }
                } {
                    Ok(_) => (),
                    Err(e) => tracing::error!(%e, "error sending"),
                }
            }
        });

        Ok(Self {
            mac_address,
            tx,
            socket: iface,
        })
    }

    pub fn get_channel(&self) -> Receiver<Arc<[u8]>> {
        let (inner_transmit, rx) = tokio::sync::mpsc::channel(1024);

        let sockc = self.socket.clone();
        tokio::spawn(async move {
            loop {
                let sockc = sockc.clone();
                let Ok(Ok(buf)) = tokio::task::spawn_blocking(move || {
                    let mut buf = uninit_array![u8; 1500];
                    let (n, _) = sockc.recv_from(&mut buf)?;
                    let buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
                    let buf = buf[..n].into();
                    Ok::<Arc<[u8]>, Error>(buf)
                })
                .await
                else {
                    break;
                };
                let _ = inner_transmit.send(buf).await;
            }
        });
        rx
    }
}
