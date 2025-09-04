use mac_address::MacAddress;
use std::collections::BinaryHeap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::time::Instant as TokioInstant;

// Trait implemented by unit/integration tests to verify packets observed by the Hub.
// Implementors can parse and assert whatever they need and set flags accordingly.
pub trait HubCheck: Send + Sync + 'static {
    fn on_packet(&self, from_idx: usize, data: &[u8]);
}

/// Delayed packet for mocked time simulation
#[derive(Debug)]
struct DelayedPacket {
    data: Vec<u8>,
    target_fd: i32,
    delivery_time: TokioInstant,
}

impl Eq for DelayedPacket {}

impl PartialEq for DelayedPacket {
    fn eq(&self, other: &Self) -> bool {
        self.delivery_time == other.delivery_time
    }
}

impl Ord for DelayedPacket {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse ordering for min-heap (earliest delivery time first)
        other.delivery_time.cmp(&self.delivery_time)
    }
}

impl PartialOrd for DelayedPacket {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
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
    pending_packets: Arc<Mutex<BinaryHeap<DelayedPacket>>>,
}

impl Hub {
    pub fn new(hub_fds: Vec<i32>, delays_ms: Vec<Vec<u64>>) -> Self {
        Self {
            hub_fds,
            delays_ms,
            checks: Vec::new(),
            use_mocked_time: false,
            pending_packets: Arc::new(Mutex::new(BinaryHeap::new())),
        }
    }

    /// Create a Hub that works properly with mocked Tokio time.
    /// When mocked time is used, delays are simulated by queuing packets
    /// and delivering them when the simulated time advances.
    pub fn new_with_mocked_time(hub_fds: Vec<i32>, delays_ms: Vec<Vec<u64>>) -> Self {
        Self {
            hub_fds,
            delays_ms,
            checks: Vec::new(),
            use_mocked_time: true,
            pending_packets: Arc::new(Mutex::new(BinaryHeap::new())),
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
        let pending_packets = self.pending_packets.clone();
        
        // Spawn the packet receiver task
        let hub_fds = self.hub_fds.clone();
        let delays = self.delays_ms;
        let checks = self.checks.clone();
        let pending_for_receiver = pending_packets.clone();
        tokio::spawn(async move {
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
                        
                        tracing::debug!("Hub received packet from index {} with {} bytes", i, buf.len());

                        // Queue packets with their delivery times
                        let now = tokio::time::Instant::now();
                        for (j, out_fd) in hub_fds.iter().copied().enumerate() {
                            if j == i {
                                continue;
                            }
                            let delay = Duration::from_millis(delays[i][j]);
                            let delivery_time = now + delay;
                            let packet = DelayedPacket {
                                data: buf.clone(),
                                target_fd: out_fd,
                                delivery_time,
                            };
                            
                            tracing::debug!("Hub queuing packet from {} to {} with delay {:?}, delivery at {:?}", 
                                          i, j, delay, delivery_time);
                            
                            if let Ok(mut queue) = pending_for_receiver.lock() {
                                queue.push(packet);
                            }
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
        });

        // Spawn the packet delivery task
        tokio::spawn(async move {
            loop {
                // Check for packets ready to be delivered
                let mut packets_to_deliver = Vec::new();
                let now = tokio::time::Instant::now();
                
                if let Ok(mut queue) = pending_packets.lock() {
                    while let Some(packet) = queue.peek() {
                        if packet.delivery_time <= now {
                            packets_to_deliver.push(queue.pop().unwrap());
                        } else {
                            break;
                        }
                    }
                }
                
                // Deliver ready packets
                for packet in packets_to_deliver {
                    tracing::debug!("Hub delivering packet with {} bytes to fd {}", packet.data.len(), packet.target_fd);
                    let _ = unsafe {
                        libc::send(packet.target_fd, packet.data.as_ptr() as *const _, packet.data.len(), 0)
                    };
                }
                
                tokio::task::yield_now().await;
            }
        });
    }
}
