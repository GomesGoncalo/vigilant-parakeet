// Test shim for `tokio_tun::Tun` so unit tests can exercise `Tun` without creating
// a real tun device. This is compiled only for tests.
#[cfg(test)]
mod test_tun {
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

// Stats and Mutex are only needed when the `stats` feature is enabled
#[cfg(feature = "stats")]
use crate::stats::Stats;
#[cfg(feature = "stats")]
use std::sync::Mutex;

#[cfg(not(test))]
use tokio_tun::Tun as TokioTun;
#[cfg(test)]
use self::test_tun::TokioTun;

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

    #[cfg(test)]
    mod tests {
    use super::Tun;
    use super::test_tun::TokioTun;
        use std::io::IoSlice;
        use tokio::task;

        #[tokio::test]
        async fn tun_send_and_recv_roundtrip() {
            // create the in-test TokioTun pair (a <-> b)
            let (a, b) = TokioTun::new_pair();
            let tun_a = Tun::new(a);

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
            let tun_a = Tun::new(a);

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

            let size = tun_a.send_vectored(&bufs).await.expect("send_vectored failed");
            assert_eq!(size, part1.len() + part2.len());

            let received = handle.await.expect("task panicked");
            let expected = [part1.as_ref(), part2.as_ref()].concat();
            assert_eq!(received, expected);
            assert!(tun_a.name().starts_with("tun-"));
        }

        #[tokio::test]
        async fn tun_recv_reads_data() {
            let (a, b) = TokioTun::new_pair();
            let tun_a = Tun::new(a);

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
            let tun_a = Tun::new(a);

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
            assert_eq!(after_send.transmitted_packets, before.transmitted_packets + 1);
            assert_eq!(after_send.transmitted_bytes, before.transmitted_bytes + sent as u128);

            // have peer send to this tun
            b.send_all(b"reply").await.expect("peer send_all");
            let mut buf = vec![0u8; 64];
            let recv_n = tun_a.recv(&mut buf).await.expect("recv");

            let after_recv = tun_a.stats();
            assert_eq!(after_recv.received_packets, before.received_packets + 1);
            assert_eq!(after_recv.received_bytes, before.received_bytes + recv_n as u128);
        }
    }
