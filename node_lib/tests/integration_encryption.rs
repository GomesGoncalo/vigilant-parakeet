/// End-to-end encryption integration tests.
///
/// These tests exercise the full data path:
///   OBU TAP → OBU (encrypt) → VANET → RSU (relay) → Server (decrypt) → Server TAP
///   Server TAP → Server (encrypt) → RSU (relay) → OBU (decrypt) → OBU TAP
///
/// Each test creates real UDP sockets (no mocked time) so that the Server's
/// async I/O integrates naturally with the VANET simulation.
use std::sync::Arc;
use std::time::Duration;

use node_lib::test_helpers::util::{mk_device_from_fd, mk_shim_pair, mk_socketpairs};
use obu_lib::Obu;
use rsu_lib::Rsu;
use server_lib::Server;

/// Helper: poll a fallible async condition with real-time timeout.
async fn wait_until<F, Fut>(mut check: F, timeout: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if check().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "wait_until timed out after {:?}",
            timeout
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Build RsuArgs pointing at a real server address.
fn mk_rsu_args_with_server(hello_periodicity: u32, server_port: u16) -> rsu_lib::RsuArgs {
    rsu_lib::RsuArgs {
        bind: String::new(),
        mtu: 1400,
        cloud_ip: None,
        rsu_params: rsu_lib::RsuParameters {
            hello_history: 10,
            hello_periodicity,
            cached_candidates: 3,
            server_ip: Some(std::net::Ipv4Addr::LOCALHOST),
            server_port,
        },
    }
}

/// Test that an OBU's upstream frame reaches the server TAP after decryption.
///
/// Topology: OBU ─── VANET ─── RSU ──── UDP ──── Server
///
/// With encryption enabled on both OBU and Server, the OBU encrypts its TAP
/// frame before sending upstream.  The Server must decrypt it and inject it
/// into its own TAP.
#[tokio::test]
async fn test_obu_upstream_reaches_server() -> anyhow::Result<()> {
    node_lib::init_test_tracing();

    // --- Server TUN shim ---
    let (server_tun, server_tun_peer) = mk_shim_pair();

    let server = Server::new(
        std::net::Ipv4Addr::LOCALHOST,
        0, // OS-assigned port
        "test_server".to_string(),
    )
    .with_tun(Arc::new(server_tun))
    .with_encryption(true);
    server.start().await?;
    let server_port = server
        .bound_addr()
        .await
        .expect("server should be bound")
        .port();

    // --- VANET: one RSU + one OBU via socketpair hub ---
    let (node_fds, hub_fds) = mk_socketpairs(2)?;
    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds,
        vec![vec![0, 0], vec![0, 0]],
        vec![],
    );

    let mac_rsu: mac_address::MacAddress = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06].into();
    let mac_obu: mac_address::MacAddress = [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);

    let (obu_tun, obu_tun_peer) = mk_shim_pair();

    // RSU connected to the live server; 100 ms heartbeats for fast convergence.
    let _rsu = Rsu::new(
        mk_rsu_args_with_server(100, server_port),
        Arc::new(dev_rsu),
        "test_rsu".to_string(),
    )?;

    let obu_args = obu_lib::test_helpers::mk_obu_args_encrypted();
    let obu = Obu::new(
        obu_args,
        Arc::new(obu_tun),
        Arc::new(dev_obu),
        "test_obu".to_string(),
    )?;

    // Wait for OBU to discover the RSU (real-time routing via timerfd heartbeats).
    wait_until(
        || async { obu.cached_upstream_mac().is_some() },
        Duration::from_secs(5),
    )
    .await;

    // Build a TAP Ethernet frame: dest = server dummy MAC, src = OBU TAP MAC.
    // Since the server has no obu_routes entry for "server_dummy_mac", it will
    // write the decrypted frame to its own TAP.
    let server_dummy_mac: [u8; 6] = [0x02, 0x42, 0x00, 0x00, 0x00, 0x01];
    let obu_tap_mac: [u8; 6] = [0x02, 0x42, 0x00, 0x00, 0x00, 0x02];
    let mut frame = Vec::new();
    frame.extend_from_slice(&server_dummy_mac);
    frame.extend_from_slice(&obu_tap_mac);
    frame.extend_from_slice(&[0x08, 0x00]); // IPv4 ethertype
    frame.extend_from_slice(b"hello_from_obu_to_server");

    obu_tun_peer.send_all(&frame).await?;

    // The server should decrypt the frame and deliver it to its TAP.
    let mut buf = vec![0u8; 65536];
    let n = tokio::time::timeout(Duration::from_secs(5), server_tun_peer.recv(&mut buf)).await??;

    assert_eq!(
        &buf[..n],
        frame.as_slice(),
        "server TAP should receive the original frame"
    );
    Ok(())
}

/// Regression test: OBU1 can ping OBU2 through the Server (L2 switch).
///
/// Topology: OBU1 ─┐
///                  ├── VANET ─── RSU ──── UDP ──── Server
///           OBU2 ─┘
///
/// The Server acts as an L2 switch: frames from OBU1 destined for OBU2's TAP
/// MAC are forwarded as DownstreamForward messages instead of being written to
/// the server's own TAP.
#[tokio::test]
async fn test_obu_to_obu_ping_through_server() -> anyhow::Result<()> {
    node_lib::init_test_tracing();

    // --- Server (no TAP needed — it only L2-switches between OBUs) ---
    let server = Server::new(std::net::Ipv4Addr::LOCALHOST, 0, "test_server".to_string())
        .with_encryption(true);
    server.start().await?;
    let server_port = server
        .bound_addr()
        .await
        .expect("server should be bound")
        .port();

    // --- VANET: one RSU + two OBUs ---
    let (node_fds, hub_fds) = mk_socketpairs(3)?;
    // All links symmetric with 0 ms delay.
    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds,
        vec![vec![0, 0, 0], vec![0, 0, 0], vec![0, 0, 0]],
        vec![],
    );

    let mac_rsu: mac_address::MacAddress = [0x01, 0x00, 0x00, 0x00, 0x00, 0x01].into();
    let mac_obu1: mac_address::MacAddress = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01].into();
    let mac_obu2: mac_address::MacAddress = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    let (obu1_tun, obu1_tun_peer) = mk_shim_pair();
    let (obu2_tun, obu2_tun_peer) = mk_shim_pair();

    let _rsu = Rsu::new(
        mk_rsu_args_with_server(100, server_port),
        Arc::new(dev_rsu),
        "test_rsu".to_string(),
    )?;

    let obu1 = Obu::new(
        obu_lib::test_helpers::mk_obu_args_encrypted(),
        Arc::new(obu1_tun),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )?;
    let obu2 = Obu::new(
        obu_lib::test_helpers::mk_obu_args_encrypted(),
        Arc::new(obu2_tun),
        Arc::new(dev_obu2),
        "test_obu2".to_string(),
    )?;

    // Wait for both OBUs to discover the RSU.
    wait_until(
        || async { obu1.cached_upstream_mac().is_some() },
        Duration::from_secs(5),
    )
    .await;
    wait_until(
        || async { obu2.cached_upstream_mac().is_some() },
        Duration::from_secs(5),
    )
    .await;

    // Choose explicit TAP MACs that we embed in injected frames.
    let obu1_tap_mac: [u8; 6] = [0x02, 0x42, 0x01, 0x00, 0x00, 0x01];
    let obu2_tap_mac: [u8; 6] = [0x02, 0x42, 0x02, 0x00, 0x00, 0x02];
    let server_dummy_mac: [u8; 6] = [0x02, 0x42, 0xFF, 0x00, 0x00, 0x00];

    // Step 1: OBU2 sends a frame to the server so the server learns OBU2's TAP MAC.
    let mut obu2_registration_frame = Vec::new();
    obu2_registration_frame.extend_from_slice(&server_dummy_mac); // dest (goes to server TAP)
    obu2_registration_frame.extend_from_slice(&obu2_tap_mac); // src (server learns this)
    obu2_registration_frame.extend_from_slice(&[0x08, 0x00]);
    obu2_registration_frame.extend_from_slice(b"obu2_hello_to_server");
    obu2_tun_peer.send_all(&obu2_registration_frame).await?;

    // Wait until the server has learned OBU2's route.
    wait_until(
        || {
            let s = &server;
            async move { s.obu_route_count().await >= 1 }
        },
        Duration::from_secs(5),
    )
    .await;

    // Step 2: OBU1 sends a frame destined for OBU2's TAP MAC.
    let mut frame_obu1_to_obu2 = Vec::new();
    frame_obu1_to_obu2.extend_from_slice(&obu2_tap_mac); // dest = OBU2 TAP
    frame_obu1_to_obu2.extend_from_slice(&obu1_tap_mac); // src = OBU1 TAP
    frame_obu1_to_obu2.extend_from_slice(&[0x08, 0x00]);
    frame_obu1_to_obu2.extend_from_slice(b"ping_from_obu1_to_obu2");
    obu1_tun_peer.send_all(&frame_obu1_to_obu2).await?;

    // OBU2's TAP should receive OBU1's original frame (server decrypts → re-encrypts
    // → RSU delivers → OBU2 decrypts → writes to TAP).
    let mut buf = vec![0u8; 65536];
    let n = tokio::time::timeout(Duration::from_secs(5), obu2_tun_peer.recv(&mut buf)).await??;

    assert_eq!(
        &buf[..n],
        frame_obu1_to_obu2.as_slice(),
        "OBU2 TAP should receive OBU1's original frame via server L2 switch"
    );
    Ok(())
}
