use mac_address::MacAddress;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

pub struct Hub {
    hub_fds: Vec<i32>,
    delays_ms: [[u64; 3]; 3],
    watch_up_from_idx: Option<usize>,
    watch_flag: Option<Arc<AtomicBool>>,
    watch_from_mac: Option<MacAddress>,
    watch_to_mac: Option<MacAddress>,
    watch_down_from_idx: Option<usize>,
    watch_down_flag: Option<Arc<AtomicBool>>,
}

impl Hub {
    pub fn new(hub_fds: Vec<i32>, delays_ms: [[u64; 3]; 3]) -> Self {
        Self {
            hub_fds,
            delays_ms,
            watch_up_from_idx: None,
            watch_flag: None,
            watch_from_mac: None,
            watch_to_mac: None,
            watch_down_from_idx: None,
            watch_down_flag: None,
        }
    }

    pub fn with_upstream_watch(
        mut self,
        idx: usize,
        from: MacAddress,
        to: MacAddress,
        flag: Arc<AtomicBool>,
    ) -> Self {
        self.watch_up_from_idx = Some(idx);
        self.watch_flag = Some(flag);
        self.watch_from_mac = Some(from);
        self.watch_to_mac = Some(to);
        self
    }

    pub fn with_downstream_watch(mut self, idx: usize, flag: Arc<AtomicBool>) -> Self {
        self.watch_down_from_idx = Some(idx);
        self.watch_down_flag = Some(flag);
        self
    }

    pub fn spawn(self) {
        tokio::spawn(async move {
            let hub_fds = self.hub_fds;
            let delays = self.delays_ms;
            let watch_idx = self.watch_up_from_idx;
            let watch_flag = self.watch_flag;
            let watch_from = self.watch_from_mac;
            let watch_to = self.watch_to_mac;
            let watch_down_idx = self.watch_down_from_idx;
            let watch_down_flag = self.watch_down_flag;
            loop {
                for i in 0..hub_fds.len() {
                    let mut buf = vec![0u8; 2048];
                    let n =
                        unsafe { libc::recv(hub_fds[i], buf.as_mut_ptr() as *mut _, buf.len(), 0) };
                    if n > 0 {
                        let n = n as usize;
                        buf.truncate(n);
                        // Optional observation: if configured, detect an Upstream data packet arriving from index i
                        if let (Some(idx), Some(flag)) = (watch_idx, watch_flag.as_ref()) {
                            if idx == i {
                                if let Ok(msg) =
                                    crate::messages::message::Message::try_from(&buf[..])
                                {
                                    if let crate::messages::packet_type::PacketType::Data(
                                        crate::messages::data::Data::Upstream(_),
                                    ) = msg.get_packet_type()
                                    {
                                        if watch_from
                                            .map(|m| msg.from().ok() == Some(m))
                                            .unwrap_or(true)
                                            && watch_to
                                                .map(|m| msg.to().ok() == Some(m))
                                                .unwrap_or(true)
                                        {
                                            flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                        // Optional observation: detect any Downstream data packet arriving from index i
                        if let (Some(ds_idx), Some(flag)) =
                            (watch_down_idx, watch_down_flag.as_ref())
                        {
                            if ds_idx == i {
                                if let Ok(msg) =
                                    crate::messages::message::Message::try_from(&buf[..])
                                {
                                    if let crate::messages::packet_type::PacketType::Data(
                                        crate::messages::data::Data::Downstream(_),
                                    ) = msg.get_packet_type()
                                    {
                                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
                            }
                        }

                        for (j, out_fd) in hub_fds.iter().copied().enumerate() {
                            if j == i {
                                continue;
                            }
                            let delay = Duration::from_millis(delays[i][j]);
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
}
