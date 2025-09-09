use mac_address::MacAddress;
use node_lib::{
    args::{Args, NodeType},
    server::{RsuToServerMessage, Server, ServerToRsuMessage},
    test_helpers::util::{mk_node_params, mk_shim_pair},
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

/// Integration test to verify server creation and protocol messages
#[tokio::test]
async fn test_server_creation_and_protocol() {
    node_lib::init_test_tracing();

    // Start server on localhost with an ephemeral port
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let server = Server::new(server_addr)
        .await
        .expect("Failed to start server");
    let actual_server_addr = server.local_addr().expect("Failed to get server address");

    // Verify server is bound to localhost
    assert_eq!(actual_server_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(actual_server_addr.port(), 0); // Should have been assigned a port

    // Test RSU to Server message serialization/deserialization
    let rsu_mac = MacAddress::from([1, 2, 3, 4, 5, 6]);
    let test_message = RsuToServerMessage {
        rsu_mac,
        encrypted_data: vec![1, 2, 3, 4, 5],
        original_source: MacAddress::from([7, 8, 9, 10, 11, 12]),
    };

    let wire_data = test_message.to_wire();
    let deserialized =
        RsuToServerMessage::from_wire(&wire_data, rsu_mac).expect("Failed to deserialize");

    assert_eq!(deserialized.rsu_mac, test_message.rsu_mac);
    assert_eq!(deserialized.encrypted_data, test_message.encrypted_data);
    assert_eq!(deserialized.original_source, test_message.original_source);

    // Test Server to RSU message serialization/deserialization
    let response = ServerToRsuMessage {
        decrypted_payload: vec![10, 20, 30],
        target_rsus: vec![MacAddress::from([1, 2, 3, 4, 5, 6])],
        destination_mac: MacAddress::from([255, 255, 255, 255, 255, 255]), // Broadcast
        source_mac: MacAddress::from([7, 8, 9, 10, 11, 12]),
    };

    let response_wire_data = response.to_wire();
    let response_deserialized =
        ServerToRsuMessage::from_wire(&response_wire_data).expect("Failed to deserialize response");

    assert_eq!(
        response_deserialized.decrypted_payload,
        response.decrypted_payload
    );
    assert_eq!(response_deserialized.target_rsus, response.target_rsus);
    assert_eq!(
        response_deserialized.destination_mac,
        response.destination_mac
    );
    assert_eq!(response_deserialized.source_mac, response.source_mac);

    println!("✓ Server creation and communication protocol validated");
}

/// Test that RSU requires server configuration
#[tokio::test]
async fn test_rsu_server_configuration() {
    node_lib::init_test_tracing();

    let (tun_a, _tun_b) = mk_shim_pair();
    let tun_a = Arc::new(tun_a);

    // Create a mock device for testing
    let device = Arc::new(common::device::Device::from_asyncfd_for_bench(
        [1, 2, 3, 4, 5, 6].into(),
        tokio::io::unix::AsyncFd::new(unsafe { std::os::unix::io::FromRawFd::from_raw_fd(0) })
            .unwrap(),
    ));

    // Test 1: RSU without server configuration should fail
    let args_no_server = Args {
        bind: String::from("test"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Rsu, Some(1000)),
    };

    assert!(args_no_server.node_params.server_address.is_none());
    let result = node_lib::control::rsu::Rsu::new(args_no_server, tun_a.clone(), device.clone());
    assert!(
        result.is_err(),
        "RSU creation should fail without server address"
    );

    // Test 2: RSU with server configuration should succeed
    let mut args_server = Args {
        bind: String::from("test"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Rsu, Some(1000)),
    };

    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345);
    args_server.node_params.server_address = Some(server_addr);

    let _rsu_server = node_lib::control::rsu::Rsu::new(args_server, tun_a.clone(), device.clone())
        .expect("Failed to create RSU with server configuration");

    println!("✓ RSU mandatory server configuration validated");
}
