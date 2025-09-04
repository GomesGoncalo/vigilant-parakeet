use mac_address::MacAddress;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

// Trait implemented by unit/integration tests to verify packets observed by the Hub.
// Implementors can parse and assert whatever they need and set flags accordingly.
pub trait HubCheck: Send + Sync + 'static {
    fn on_packet(&self, from_idx: usize, data: &[u8]);
}

/// Reusable checker for Upstream packets, matching index, from/to, and optional payload.
pub struct UpstreamMatchCheck {
    pub idx: usize,
    pub from: MacAddress,
    pub to: MacAddress,
    pub expected_payload: Option<Vec<u8>>, // None = don't check payload
    pub flag: Arc<AtomicBool>,
}

impl HubCheck for UpstreamMatchCheck {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        if from_idx != self.idx {
            return;
        }
        if let Ok(msg) = crate::messages::message::Message::try_from(data) {
            if let crate::messages::packet_type::PacketType::Data(
                crate::messages::data::Data::Upstream(u),
            ) = msg.get_packet_type()
            {
                if msg.from().ok() == Some(self.from) && msg.to().ok() == Some(self.to) {
                    if let Some(exp) = &self.expected_payload {
                        if u.data().as_ref() != &exp[..] {
                            return;
                        }
                    }
                    self.flag.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

/// Reusable checker for any Downstream packet observed from a given hub index.
pub struct DownstreamFromIdxCheck {
    pub idx: usize,
    pub flag: Arc<AtomicBool>,
}

impl HubCheck for DownstreamFromIdxCheck {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        if from_idx != self.idx {
            return;
        }
        if let Ok(msg) = crate::messages::message::Message::try_from(data) {
            if let crate::messages::packet_type::PacketType::Data(
                crate::messages::data::Data::Downstream(_),
            ) = msg.get_packet_type()
            {
                self.flag.store(true, Ordering::SeqCst);
            }
        }
    }
}

pub struct Hub {
    hub_fds: Vec<i32>,
    delays_ms: Vec<Vec<u64>>,
    checks: Vec<Arc<dyn HubCheck>>,
    use_mocked_time: bool,
}

impl Hub {
    pub fn new(hub_fds: Vec<i32>, delays_ms: Vec<Vec<u64>>) -> Self {
        Self {
            hub_fds,
            delays_ms,
            checks: Vec::new(),
            use_mocked_time: false,
        }
    }

    /// Create a Hub that works properly with mocked Tokio time.
    /// When mocked time is used, delays are simulated using tokio::time::sleep
    /// which respects mocked time advancement.
    pub fn new_with_mocked_time(hub_fds: Vec<i32>, delays_ms: Vec<Vec<u64>>) -> Self {
        Self {
            hub_fds,
            delays_ms,
            checks: Vec::new(),
            use_mocked_time: true,
        }
    }

    /// Add a packet check to be invoked for every observed packet.
    pub fn add_check(mut self, check: Arc<dyn HubCheck>) -> Self {
        self.checks.push(check);
        self
    }

    /// Replace the full list of checks.
    pub fn with_checks(mut self, checks: Vec<Arc<dyn HubCheck>>) -> Self {
        self.checks = checks;
        self
    }

    pub fn spawn(self) {
        if self.use_mocked_time {
            self.spawn_with_mocked_time();
        } else {
            self.spawn_with_real_time();
        }
    }

    fn spawn_with_real_time(self) {
        tokio::spawn(async move {
            let hub_fds = self.hub_fds;
            let delays = self.delays_ms;
            let checks = self.checks;
            loop {
                for i in 0..hub_fds.len() {
                    let mut buf = vec![0u8; 2048];
                    let n =
                        unsafe { libc::recv(hub_fds[i], buf.as_mut_ptr() as *mut _, buf.len(), 0) };
                    if n > 0 {
                        let n = n as usize;
                        buf.truncate(n);
                        // Invoke user-provided checks
                        for check in &checks {
                            check.on_packet(i, &buf);
                        }

                        for (j, out_fd) in hub_fds.iter().copied().enumerate() {
                            if j == i {
                                continue;
                            }
                            // Safely index into the delays matrix; default to 0ms when absent.
                            let delay_ms = delays
                                .get(i)
                                .and_then(|r| r.get(j))
                                .copied()
                                .unwrap_or(0u64);
                            let delay = Duration::from_millis(delay_ms);
                            let data = buf.clone();
                            tokio::spawn(async move {
                                if delay.as_millis() > 0 {
                                    tokio::time::sleep(delay).await;
                                }
                                let _ = unsafe {
                                    libc::send(out_fd, data.as_ptr() as *const _, data.len(), 0)
                                };
                            });
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
        });
    }

    fn spawn_with_mocked_time(self) {
        // In mocked time mode, we use tokio::time::sleep directly with the delay
        // instead of trying to track absolute delivery times
        tokio::spawn(async move {
            let hub_fds = self.hub_fds;
            let delays = self.delays_ms;
            let checks = self.checks;
            loop {
                for i in 0..hub_fds.len() {
                    let mut buf = vec![0u8; 2048];
                    let n = unsafe {
                        libc::recv(
                            hub_fds[i],
                            buf.as_mut_ptr() as *mut _,
                            buf.len(),
                            libc::MSG_DONTWAIT,
                        )
                    };
                    if n > 0 {
                        let n = n as usize;
                        buf.truncate(n);
                        // Invoke user-provided checks
                        for check in &checks {
                            check.on_packet(i, &buf);
                        }

                        tracing::debug!(
                            "Hub received packet from index {} with {} bytes",
                            i,
                            buf.len()
                        );

                        // Forward packets with delays
                        for (j, out_fd) in hub_fds.iter().copied().enumerate() {
                            if j == i {
                                continue;
                            }
                            let delay_ms = delays[i][j];
                            let delay = Duration::from_millis(delay_ms);
                            let data = buf.clone();

                            tracing::debug!(
                                "Hub spawning delivery task from {} to {} with delay {:?}",
                                i,
                                j,
                                delay
                            );

                            tokio::spawn(async move {
                                if delay.as_millis() > 0 {
                                    tokio::time::sleep(delay).await;
                                }
                                tracing::debug!("Hub delivering packet with delay {:?}", delay);
                                let _ = unsafe {
                                    libc::send(out_fd, data.as_ptr() as *const _, data.len(), 0)
                                };
                            });
                        }
                    }
                }

                // Use a small sleep to allow tokio to advance mocked time
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        });
    }
}
