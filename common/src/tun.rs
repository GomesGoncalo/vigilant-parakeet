// Test shim for `tokio_tun::Tun` so unit tests can exercise `Tun` without creating
// a real tun device. The module is exposed so downstream integration tests can
// use the shim directly; the crate still controls whether `TokioTun` aliases
// to this shim via feature flags.
pub mod test_tun {
    use std::io::{self, IoSlice};
    use tokio::sync::mpsc;

    pub struct TokioTun {
        tx: mpsc::Sender<Vec<u8>>,
        rx: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
        name: String,
    }

    impl TokioTun {
        pub fn new_pair() -> (Self, Self) {
            let (a_tx, a_rx) = mpsc::channel(8);
            let (b_tx, b_rx) = mpsc::channel(8);
            let a = Self {
                tx: a_tx,
                rx: tokio::sync::Mutex::new(b_rx),
                name: "tun-a".to_string(),
            };
            let b = Self {
                tx: b_tx,
                rx: tokio::sync::Mutex::new(a_rx),
                name: "tun-b".to_string(),
            };
            (a, b)
        }

        pub async fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
            let mut v = Vec::new();
            for b in bufs {
                v.extend_from_slice(b);
            }
            let len = v.len();
            self.tx
                .send(v)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "send failed"))?;
            Ok(len)
        }

        // Implement recv taking &self; the receiver is stored behind a tokio Mutex so
        // we can await and call `recv` on the locked receiver.
        pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
            let mut lock = self.rx.lock().await;
            match lock.recv().await {
                Some(v) => {
                    let n = std::cmp::min(v.len(), buf.len());
                    buf[..n].copy_from_slice(&v[..n]);
                    Ok(n)
                }
                None => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "recv closed")),
            }
        }

        pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
            self.tx
                .send(buf.to_vec())
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "send failed"))?;
            Ok(())
        }

        pub fn name(&self) -> &str {
            &self.name
        }
    }
}

// Bring common IO types into scope for both test and non-test builds
use std::io::{self, IoSlice};

// Stats and AtomicStats are only needed when the `stats` feature is enabled
#[cfg(feature = "stats")]
use crate::stats::{AtomicStats, Stats};

// Use a conditional alias so `TokioTun` refers to the test shim when running
// tests or when the `test_helpers` feature is enabled, otherwise use the
// real `tokio_tun::Tun` type.
#[cfg(any(test, feature = "test_helpers"))]
pub use test_tun::TokioTun;

#[cfg(not(any(test, feature = "test_helpers")))]
pub use tokio_tun::Tun as TokioTun;

pub struct Tun {
    inner: TunInner,
    #[cfg(feature = "stats")]
    stats: AtomicStats,
}

// Internal enum to store either the real runtime tun or the test shim.
enum TunInner {
    #[cfg(not(target_family = "wasm"))]
    Real(tokio_tun::Tun),
    Shim(test_tun::TokioTun),
}

impl Tun {
    pub fn new(tun: TokioTun) -> Self {
        // TokioTun is aliased depending on cfg; delegate to the correct
        // concrete constructor.
        #[cfg(any(test, feature = "test_helpers"))]
        {
            Tun::new_shim(tun)
        }

        #[cfg(not(any(test, feature = "test_helpers")))]
        {
            Tun::new_real(tun)
        }
    }

    /// Construct a `Tun` from the test shim type.
    pub fn new_shim(t: test_tun::TokioTun) -> Self {
        Self {
            inner: TunInner::Shim(t),
            #[cfg(feature = "stats")]
            stats: AtomicStats::new(),
        }
    }

    /// Construct a `common::tun::Tun` from a real `tokio_tun::Tun` instance.
    ///
    /// This helper is intentionally provided so callers don't need to depend on
    /// the `test_helpers` feature to convert the concrete runtime type into
    /// the wrapper. It is available for non-wasm builds.
    /// Construct a `Tun` from a real `tokio_tun::Tun`.
    #[cfg(not(target_family = "wasm"))]
    pub fn new_real(t: tokio_tun::Tun) -> Self {
        Self {
            inner: TunInner::Real(t),
            #[cfg(feature = "stats")]
            stats: AtomicStats::new(),
        }
    }

    /// Construct a `common::tun::Tun` from the test shim `TokioTun`.
    ///
    /// Provided so tests or integration harnesses can create a `Tun` from the
    /// shim without needing to rely on `From` impls that change with cfg
    /// toggles. This is available when the test shim is compiled in.
    // Keep a convenience alias available for code that used the previous
    // helper name.
    #[cfg(feature = "test_helpers")]
    pub fn from_shim_tun(t: test_tun::TokioTun) -> Self {
        Tun::new_shim(t)
    }

    #[cfg(feature = "stats")]
    pub fn stats(&self) -> Stats {
        self.stats.snapshot()
    }

    pub async fn send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let size = match &self.inner {
            #[cfg(not(target_family = "wasm"))]
            TunInner::Real(t) => {
                // Fallback: tokio_tun::Tun may not support vectored sends on all versions.
                // Flatten the IoSlices and use send_all.
                let total: usize = bufs.iter().map(|s| s.len()).sum();
                let mut v = Vec::with_capacity(total);
                for s in bufs {
                    v.extend_from_slice(s);
                }
                t.send_all(&v).await?;
                total
            }
            TunInner::Shim(s) => s.send_vectored(bufs).await?,
        };
        #[cfg(feature = "stats")]
        {
            self.stats.record_transmit(size);
        }
        Ok(size)
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let size = match &self.inner {
            #[cfg(not(target_family = "wasm"))]
            TunInner::Real(t) => t.recv(buf).await?,
            TunInner::Shim(s) => s.recv(buf).await?,
        };
        #[cfg(feature = "stats")]
        {
            self.stats.record_receive(size);
        }
        Ok(size)
    }

    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        match &self.inner {
            #[cfg(not(target_family = "wasm"))]
            TunInner::Real(t) => t.send_all(buf).await?,
            TunInner::Shim(s) => s.send_all(buf).await?,
        };
        #[cfg(feature = "stats")]
        {
            self.stats.record_transmit(buf.len());
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        match &self.inner {
            #[cfg(not(target_family = "wasm"))]
            TunInner::Real(t) => t.name(),
            TunInner::Shim(s) => s.name(),
        }
    }
}

// Provide convenient conversion from concrete tokio_tun::Tun into our wrapper
#[cfg(not(any(test, feature = "test_helpers")))]
impl From<tokio_tun::Tun> for Tun {
    fn from(t: tokio_tun::Tun) -> Self {
        Tun::new(t)
    }
}

// When the test shim is active, allow converting from the shim type as well
#[cfg(any(test, feature = "test_helpers"))]
impl From<test_tun::TokioTun> for Tun {
    fn from(t: test_tun::TokioTun) -> Self {
        Tun::new(t)
    }
}

#[cfg(test)]
mod tests {
    use super::test_tun::TokioTun;
    use super::Tun;
    use std::io::IoSlice;
    use tokio::task;

    #[tokio::test]
    async fn tun_send_and_recv_roundtrip() {
        // create the in-test TokioTun pair (a <-> b)
        let (a, b) = TokioTun::new_pair();
        let tun_a = Tun::new_shim(a);

        // spawn a task that receives from the peer side
        let handle = task::spawn(async move {
            let mut buf = vec![0u8; 128];
            let n = b.recv(&mut buf).await.expect("recv failed");
            buf.truncate(n);
            buf
        });

        // send data via Tun::send_all and verify the peer receives it
        let payload = b"hello tun";
        tun_a.send_all(payload).await.expect("send_all failed");

        let received = handle.await.expect("task panicked");
        assert_eq!(received.as_slice(), payload);
    }

    #[tokio::test]
    async fn tun_send_vectored_and_name() {
        let (a, b) = TokioTun::new_pair();
        let tun_a = Tun::new_shim(a);

        // prepare vectored buffers
        let part1 = b"hello ";
        let part2 = b"world";
        let bufs = [IoSlice::new(part1), IoSlice::new(part2)];

        // spawn reader that collects the incoming bytes
        let handle = task::spawn(async move {
            let mut buf = vec![0u8; 128];
            let n = b.recv(&mut buf).await.expect("recv failed");
            buf.truncate(n);
            buf
        });

        let size = tun_a
            .send_vectored(&bufs)
            .await
            .expect("send_vectored failed");
        assert_eq!(size, part1.len() + part2.len());

        let received = handle.await.expect("task panicked");
        let expected = [part1.as_ref(), part2.as_ref()].concat();
        assert_eq!(received, expected);
        assert!(tun_a.name().starts_with("tun-"));
    }

    #[tokio::test]
    async fn tun_recv_reads_data() {
        let (a, b) = TokioTun::new_pair();
        let tun_a = Tun::new_shim(a);

        // spawn a task that sends data from the peer side
        let sender = task::spawn(async move {
            let payload = b"reply";
            b.send_all(payload).await.expect("peer send_all failed");
        });

        let mut buf = vec![0u8; 64];
        let n = tun_a.recv(&mut buf).await.expect("recv failed");
        assert_eq!(&buf[..n], b"reply");

        sender.await.expect("sender panicked");
    }

    #[cfg(feature = "stats")]
    #[tokio::test]
    async fn tun_stats_increment_on_send_and_recv() {
        use std::io::IoSlice;

        let (a, b) = TokioTun::new_pair();
        let tun_a = Tun::new_shim(a);

        let before = tun_a.stats();
        assert_eq!(before.transmitted_packets, 0);
        assert_eq!(before.transmitted_bytes, 0);

        // send vectored
        let part1 = b"hi ";
        let part2 = b"there";
        let bufs = [IoSlice::new(part1), IoSlice::new(part2)];
        let sent = tun_a.send_vectored(&bufs).await.expect("send_vectored");
        assert_eq!(sent, part1.len() + part2.len());

        let after_send = tun_a.stats();
        assert_eq!(
            after_send.transmitted_packets,
            before.transmitted_packets + 1
        );
        assert_eq!(
            after_send.transmitted_bytes,
            before.transmitted_bytes + sent as u128
        );

        // have peer send to this tun
        b.send_all(b"reply").await.expect("peer send_all");
        let mut buf = vec![0u8; 64];
        let recv_n = tun_a.recv(&mut buf).await.expect("recv");

        let after_recv = tun_a.stats();
        assert_eq!(after_recv.received_packets, before.received_packets + 1);
        assert_eq!(
            after_recv.received_bytes,
            before.received_bytes + recv_n as u128
        );
    }
}
