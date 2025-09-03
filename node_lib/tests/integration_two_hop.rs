use common::device::{Device, DeviceIo};
use common::tun::test_tun::TokioTun;
use common::tun::Tun;
use node_lib::args::{Args, NodeParameters, NodeType};
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use std::os::unix::io::FromRawFd;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::unix::AsyncFd;

/// Simple 3-port hub that forwards frames from any port to all others with
/// configurable per-edge delays.
struct Hub {
    hub_fds: Vec<i32>,
    delays_ms: [[u64; 3]; 3],
    watch_upstream_from_idx: Option<usize>,
    watch_flag: Option<Arc<AtomicBool>>,
    watch_from_mac: Option<mac_address::MacAddress>,
    watch_to_mac: Option<mac_address::MacAddress>,
}

impl Hub {
    fn new(hub_fds: Vec<i32>, delays_ms: [[u64; 3]; 3]) -> Self {
        Self {
            hub_fds,
            delays_ms,
            watch_upstream_from_idx: None,
            watch_flag: None,
            watch_from_mac: None,
            watch_to_mac: None,
        }
    }

    fn with_upstream_watch(
        mut self,
        idx: usize,
        from: mac_address::MacAddress,
        to: mac_address::MacAddress,
        flag: Arc<AtomicBool>,
    ) -> Self {
        self.watch_upstream_from_idx = Some(idx);
        self.watch_flag = Some(flag);
        self.watch_from_mac = Some(from);
        self.watch_to_mac = Some(to);
        self
    }

    fn spawn(self) {
        tokio::spawn(async move {
            let hub_fds = self.hub_fds;
            let delays = self.delays_ms;
            let watch_idx = self.watch_upstream_from_idx;
            let watch_flag = self.watch_flag;
            let watch_from = self.watch_from_mac;
            let watch_to = self.watch_to_mac;
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
                                    node_lib::messages::message::Message::try_from(&buf[..])
                                {
                                    if let node_lib::messages::packet_type::PacketType::Data(
                                        node_lib::messages::data::Data::Upstream(_),
                                    ) = msg.get_packet_type()
                                    {
                                        if watch_from
                                            .map(|m| msg.from().ok() == Some(m))
                                            .unwrap_or(true)
                                            && watch_to
                                                .map(|m| msg.to().ok() == Some(m))
                                                .unwrap_or(true)
                                        {
                                            flag.store(true, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                        for j in 0..hub_fds.len() {
                            if j == i {
                                continue;
                            }
                            let out_fd = hub_fds[j];
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
                // small yield to avoid busy loop
                tokio::task::yield_now().await;
            }
        });
    }
}

#[tokio::test]
async fn rsu_and_two_obus_choose_two_hop_when_direct_has_higher_latency() {
    node_lib::init_test_tracing();

    // Create 3 TUNs (one per node). Keep OBU2's peer so we can inject upstream traffic reliably.
    let (tun_rsu_a, _) = TokioTun::new_pair();
    let (tun_obu1_a, _) = TokioTun::new_pair();
    let (tun_obu2_a, tun_obu2_b) = TokioTun::new_pair();
    let tun_rsu = Tun::new_shim(tun_rsu_a);
    let tun_obu1 = Tun::new_shim(tun_obu1_a);
    let tun_obu2 = Tun::new_shim(tun_obu2_a);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let mut node_fds = [0; 3];
    let mut hub_fds = [0; 3];
    for i in 0..3 {
        let mut fds = [0; 2];
        unsafe {
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
            assert_eq!(r, 0, "socketpair failed");
            let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            let _ = libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
        }
        node_fds[i] = fds[0];
        hub_fds[i] = fds[1];
    }

    // Wrap node ends as Devices
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Spawn the hub with delay matrix: index 0=RSU, 1=OBU1, 2=OBU2
    // Make direct path RSU->OBU2 high latency (50ms), RSU<->OBU1 and OBU1<->OBU2 low (2ms)
    let delays = [[0, 2, 50], [2, 0, 2], [50, 2, 0]];
    let saw_forward_to_obu1 = Arc::new(AtomicBool::new(false));
    Hub::new(hub_fds.to_vec(), delays)
        // watch index 2 (OBU2's inbound to the hub) for an Upstream packet specifically from OBU2 headed to OBU1
        .with_upstream_watch(2, mac_obu2, mac_obu1, saw_forward_to_obu1.clone())
        .spawn();

    let dev_rsu = Device::from_asyncfd_for_bench(
        mac_rsu,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[0]) }).unwrap(),
    );
    let dev_obu1 = Device::from_asyncfd_for_bench(
        mac_obu1,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[1]) }).unwrap(),
    );
    let dev_obu2 = Device::from_asyncfd_for_bench(
        mac_obu2,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[2]) }).unwrap(),
    );

    // Build Args
    let args_rsu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Rsu,
            hello_history: 10,
            hello_periodicity: Some(50),
        },
    };
    let args_obu1 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Obu,
            hello_history: 10,
            hello_periodicity: None,
        },
    };
    let args_obu2 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Obu,
            hello_history: 10,
            hello_periodicity: None,
        },
    };

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("rsu new");
    let _obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("obu1 new");
    let tun_obu2_arc = Arc::new(tun_obu2);
    let obu2 = Obu::new(args_obu2, tun_obu2_arc, Arc::new(dev_obu2)).expect("obu2 new");

    // Wait for OBU2 to cache upstream route; expect it to be OBU1 (two-hop path)
    let mut cached = None;
    for _ in 0..100 {
        // up to ~10s
        tokio::time::sleep(Duration::from_millis(100)).await;
        cached = obu2.cached_upstream_mac();
        if cached.is_some() {
            break;
        }
    }
    assert!(cached.is_some(), "OBU2 did not cache an upstream");
    assert_eq!(
        cached.unwrap(),
        mac_obu1,
        "OBU2 should prefer two-hop path via OBU1 when direct link has higher latency"
    );

    // Trigger an upstream send by writing on the peer end of OBU2's TUN; the session task should forward it.
    let payload = b"test payload";
    let peer = Tun::new_shim(tun_obu2_b);
    let _ = peer.send_all(payload).await;

    // Wait up to ~2s for the hub to observe the upstream packet
    for _ in 0..20 {
        if saw_forward_to_obu1.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // This assertion is soft; the primary assertion is the cached upstream.
    assert!(saw_forward_to_obu1.load(Ordering::SeqCst));
}
