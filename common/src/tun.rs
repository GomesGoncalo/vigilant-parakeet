#[cfg(feature = "stats")]
use crate::stats::Stats;
use std::{
    io::{self, IoSlice},
    sync::Mutex,
};
use tokio_tun::Tun as TokioTun;

pub struct Tun {
    tun: TokioTun,
    #[cfg(feature = "stats")]
    stats: Mutex<Stats>,
}

impl Tun {
    pub fn new(tun: TokioTun) -> Self {
        Self {
            tun,
            #[cfg(feature = "stats")]
            stats: Stats::default().into(),
        }
    }

    #[cfg(feature = "stats")]
    pub fn stats(&self) -> Stats {
        *self.stats.lock().unwrap()
    }

    pub async fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let size = self.tun.send_vectored(bufs).await?;
        #[cfg(feature = "stats")]
        {
            let mut guard = self.stats.lock().unwrap();
            guard.transmitted_packets += 1;
            guard.transmitted_bytes += size as u128;
        }
        Ok(size)
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.tun.recv(buf).await?;
        #[cfg(feature = "stats")]
        {
            let mut guard = self.stats.lock().unwrap();
            guard.received_packets += 1;
            guard.received_bytes += size as u128;
        }
        Ok(size)
    }

    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        self.tun.send_all(buf).await?;
        #[cfg(feature = "stats")]
        {
            let mut guard = self.stats.lock().unwrap();
            guard.transmitted_packets += 1;
            guard.transmitted_bytes += buf.len() as u128;
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        self.tun.name()
    }
}
