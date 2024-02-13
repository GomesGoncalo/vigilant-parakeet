use std::{io::IoSlice, os::fd::AsRawFd, sync::Arc};

use anyhow::{Context, Result};
use libc::{sockaddr, sockaddr_ll, AF_PACKET};
use nix::sys::socket::{self, LinkAddr, SockaddrLike};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::{mpsc::Sender, watch::Receiver};
use uninit::uninit_array;

const ETH_P_ALL: u16 = 0x0003;

pub struct Device {}

pub enum OutgoingMessage {
    Simple(Vec<u8>),
    Vectored(Vec<Vec<u8>>),
}

impl Device {
    pub fn new(interface: &str) -> Result<(Self, Receiver<Arc<Vec<u8>>>, Sender<OutgoingMessage>)> {
        let iface = Arc::new(Socket::new(
            Domain::PACKET,
            Type::RAW,
            Some(Protocol::from(ETH_P_ALL.to_be() as i32)),
        )?);

        let idx = nix::net::if_::if_nametoindex(interface)?;
        let mut saddr: sockaddr_ll = unsafe { std::mem::zeroed() };
        saddr.sll_family = AF_PACKET as u16;
        saddr.sll_ifindex = idx as i32;
        let p_saddr = &mut saddr as *mut sockaddr_ll;
        let p_saddr: &mut sockaddr = unsafe { std::mem::transmute(p_saddr) };
        let storage =
            unsafe { LinkAddr::from_raw(p_saddr, Some(std::mem::size_of::<sockaddr_ll>() as u32)) }
                .context("casting link storage")?;
        let _ = socket::bind(iface.as_raw_fd(), &storage);
        let (itx, rx) = tokio::sync::watch::channel(Arc::new(Vec::default()));
        let (tx, mut irx): (Sender<_>, tokio::sync::mpsc::Receiver<_>) =
            tokio::sync::mpsc::channel(1024);

        let sockc = iface.clone();
        tokio::task::spawn_blocking(move || loop {
            let mut buf = uninit_array![u8; 1500];
            let n = match sockc.recv_from(&mut buf) {
                Ok((n, _)) => n,
                _ => break,
            };

            let received = buf
                .iter()
                .take(n)
                .map(|mu| unsafe { mu.assume_init() })
                .collect::<Vec<_>>();

            let _ = itx.send(Arc::new(received));
        });

        tokio::spawn(async move {
            while let Some(i) = irx.recv().await {
                match match i {
                    OutgoingMessage::Simple(msg) => iface.send(&msg),
                    OutgoingMessage::Vectored(vec_msg) => {
                        let vec: Vec<IoSlice> = vec_msg.iter().map(|x| IoSlice::new(&x)).collect();
                        iface.send_vectored(&vec)
                    }
                } {
                    Ok(_) => (),
                    Err(e) => tracing::error!(e = %e, "error sending"),
                }
            }
        });

        Ok((Self {}, rx, tx))
    }
}
