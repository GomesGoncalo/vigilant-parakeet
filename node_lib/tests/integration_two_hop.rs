use common::device::{Device, DeviceIo};
use common::tun::test_tun::TokioTun;
use common::tun::Tun;
use node_lib::args::{Args, NodeParameters, NodeType};
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::Hub;
use std::os::unix::io::FromRawFd;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::unix::AsyncFd;

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
            cached_candidates: 3,
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
            cached_candidates: 3,
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
            cached_candidates: 3,
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

/// End-to-end: OBU2 "pings" RSU two hops away. We inject a request frame into
/// OBU2's TUN (dest=RSU MAC, src=OBU2 MAC, payload=bytes) and expect it to reach
/// RSU's TUN. Then we inject a reply from RSU's TUN (dest=OBU2 MAC, src=RSU MAC)
/// and expect OBU2's TUN to receive the reply payload. This verifies both
/// directions succeed across the two-hop route selection.
#[tokio::test]
async fn two_hop_ping_roundtrip_obu2_to_rsu() {
    node_lib::init_test_tracing();

    // Create TUNs. Keep peers for RSU and OBU2 to inject/observe frames.
    let (tun_rsu_a, tun_rsu_b) = TokioTun::new_pair();
    let (tun_obu1_a, _tun_obu1_b) = TokioTun::new_pair();
    let (tun_obu2_a, tun_obu2_b) = TokioTun::new_pair();
    let tun_rsu = Tun::new_shim(tun_rsu_a);
    let tun_rsu_peer = Tun::new_shim(tun_rsu_b);
    let tun_obu1 = Tun::new_shim(tun_obu1_a);
    let tun_obu2 = Tun::new_shim(tun_obu2_a);
    let tun_obu2_peer = Tun::new_shim(tun_obu2_b);

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

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays = [[0, 2, 50], [2, 0, 2], [50, 2, 0]];
    let saw_downstream_from_rsu = Arc::new(AtomicBool::new(false));
    Hub::new(hub_fds.to_vec(), delays)
        .with_downstream_watch(0, saw_downstream_from_rsu.clone()) // RSU index = 0
        .spawn();

    // Wrap node ends as Devices
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
            cached_candidates: 3,
        },
    };
    let args_obu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Obu,
            hello_history: 10,
            hello_periodicity: None,
            cached_candidates: 3,
        },
    };

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("rsu new");
    let _obu1 =
        Obu::new(args_obu.clone(), Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("obu1 new");
    let obu2 = Obu::new(args_obu, Arc::new(tun_obu2), Arc::new(dev_obu2)).expect("obu2 new");

    // Wait for OBU2 to cache upstream via OBU1 (two-hop path preferred)
    let mut cached = None;
    for _ in 0..100 {
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
        "OBU2 should pick OBU1 as upstream"
    );

    // Prime RSU's client cache with a mapping for RSU's own MAC -> RSU node MAC
    // by sending any frame from RSU's TUN (process_tap_traffic stores `from` -> device.mac).
    let mut prime = Vec::new();
    prime.extend_from_slice(&[255u8; 6]); // dest broadcast
    prime.extend_from_slice(&mac_rsu.bytes()); // from = RSU
    prime.extend_from_slice(b"prime");
    tun_rsu_peer
        .send_all(&prime)
        .await
        .expect("prime send to RSU tun");
    // Give a moment for RSU to process and store mapping
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Compose a "ping" request frame from OBU2 destined to RSU
    let payload_req = b"ping-req";
    let mut req = Vec::new();
    req.extend_from_slice(&mac_rsu.bytes()); // to
    req.extend_from_slice(&mac_obu2.bytes()); // from
    req.extend_from_slice(payload_req); // body

    // Send request into OBU2's TUN (session will forward upstream over two hops)
    tun_obu2_peer
        .send_all(&req)
        .await
        .expect("send ping req to OBU2 tun");

    // Expect RSU's TUN to receive the full upstream request frame (to+from+payload)
    let mut buf = vec![0u8; 256];
    let mut got_req_at_rsu = false;
    for _ in 0..100 {
        if let Ok(n) =
            tokio::time::timeout(Duration::from_millis(100), tun_rsu_peer.recv(&mut buf)).await
        {
            let n = n.expect("rsu peer recv ok");
            if n >= req.len() && &buf[..req.len()] == &req[..] {
                got_req_at_rsu = true;
                break;
            }
        }
    }
    assert!(got_req_at_rsu, "RSU did not receive ping request on TUN");

    // Give RSU additional time to ensure it has a route to OBU2
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Now craft and send a reply from RSU back to OBU2 via RSU's TUN
    let payload_rep = b"ping-rep";
    let mut rep = Vec::new();
    rep.extend_from_slice(&mac_obu2.bytes()); // to
    rep.extend_from_slice(&mac_rsu.bytes()); // from
    rep.extend_from_slice(payload_rep);
    tun_rsu_peer
        .send_all(&rep)
        .await
        .expect("send ping reply from RSU tun");

    // Wait for hub to observe a Downstream packet from RSU (port index 0)
    for _ in 0..50 {
        if saw_downstream_from_rsu.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!(
        "hub saw downstream from RSU: {}",
        saw_downstream_from_rsu.load(Ordering::SeqCst)
    );

    // Wait for the hub to observe a Downstream frame emitted from RSU before expecting OBU2's TUN
    for _ in 0..50 {
        if saw_downstream_from_rsu.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Expect OBU2's TUN to receive the full downstream reply frame (to+from+payload)
    let mut got_rep_at_obu2 = false;
    let mut rx = vec![0u8; 256];
    let mut seen_samples: Vec<Vec<u8>> = Vec::new();
    for _ in 0..150 {
        if let Ok(n) =
            tokio::time::timeout(Duration::from_millis(100), tun_obu2_peer.recv(&mut rx)).await
        {
            let n = n.expect("obu2 peer recv ok");
            let snapshot = rx[..n].to_vec();
            if seen_samples.len() < 8 {
                seen_samples.push(snapshot.clone());
            }
            if n >= rep.len() && &snapshot[..rep.len()] == &rep[..] {
                got_rep_at_obu2 = true;
                break;
            }
        }
    }
    if !got_rep_at_obu2 {
        println!("received {} frames at OBU2 TUN:", seen_samples.len());
        for (i, s) in seen_samples.iter().enumerate() {
            let preview: Vec<String> = s.iter().take(24).map(|b| format!("{:02x}", b)).collect();
            println!(
                "frame {}: len={}, head={}...",
                i,
                s.len(),
                preview.join(" ")
            );
        }
    }
    assert!(got_rep_at_obu2, "OBU2 did not receive ping reply on TUN");
}
