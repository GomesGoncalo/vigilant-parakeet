#[cfg(test)]
mod node_tests {
    use anyhow::Result;
    use common::device::Device;
    use mac_address::MacAddress;
    use node_lib::messages::{
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };

    // Helper to make a Device backed by a non-blocking pipe writer fd that we can close to force errors
    fn mk_device_from_pipe(mac: MacAddress) -> (Device, i32, i32) {
        use node_lib::test_helpers::util::{close_fd, mk_pipe_nonblocking};
        use tokio::io::unix::AsyncFd;
        let (reader_fd, writer_fd) = mk_pipe_nonblocking().expect("pipe");
        // Safety: transfer ownership of writer_fd into Device
        let async_fd = AsyncFd::new(unsafe { common::device::DeviceIo::from_raw_fd(writer_fd) })
            .expect("AsyncFd");
        let device = Device::from_asyncfd_for_bench(mac, async_fd);
        (device, reader_fd, writer_fd)
    }

    #[tokio::test]
    async fn handle_messages_tap_path_sends_to_tun() -> Result<()> {
        let (tun, tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = std::sync::Arc::new(tun);
        let dev_mac: MacAddress = [1u8; 6].into();
        let (device, reader_fd, writer_fd) = mk_device_from_pipe(dev_mac);
        let dev = std::sync::Arc::new(device);

        // Prepare a TapFlat reply: arbitrary payload
        let payload = b"hello".to_vec();
        let msgs = vec![obu_lib::control::node::ReplyType::TapFlat(payload.clone())];

        // Spawn handle_messages and ensure peer receives the same bytes
        let routing: Option<std::sync::Arc<std::sync::RwLock<obu_lib::control::routing::Routing>>> = None;
        obu_lib::control::node::handle_messages(msgs, &tun, &dev, routing)
            .await
            .expect("handle_messages");

        // Read from tun_peer to confirm delivery
        let mut buf = vec![0u8; 64];
        let n = tun_peer.recv(&mut buf).await.expect("recv from peer");
        assert_eq!(&buf[..n], b"hello");

        // Cleanup
        let _ = unsafe { libc::close(reader_fd) };
        let _ = unsafe { libc::close(writer_fd) };
        Ok(())
    }

    #[tokio::test]
    async fn handle_messages_wire_send_error_triggers_failover() -> Result<()> {
        use node_lib::messages::data::ToDownstream;
        use node_lib::messages::packet_type::PacketType;
        use obu_lib::control::routing::Routing;
        use tokio::time::{Duration, Instant};

        // Build routing with a cached upstream so that a Wire destined to that MAC will match
        let args = obu_lib::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: obu_lib::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing");
        let rsu: MacAddress = [9u8; 6].into();
        // Seed a candidate list with two entries and set primary
        let primary: MacAddress = [2u8; 6].into();
        let backup: MacAddress = [3u8; 6].into();
        routing
            .cached_candidates
            .store(Some(std::sync::Arc::new(vec![primary, backup])));
        routing.cached_upstream.store(Some(primary.into()));
        routing.cached_source.store(Some(rsu.into()));
        let routing = std::sync::Arc::new(std::sync::RwLock::new(routing));

        // Device that will fail on send: close writer end to cause send_vectored error
        let dev_mac: MacAddress = [1u8; 6].into();
        let (device, reader_fd, writer_fd) = mk_device_from_pipe(dev_mac);
        // Close the reader so writes fail (EPIPE)
        let _ = unsafe { libc::close(reader_fd) };
        let dev = std::sync::Arc::new(device);

        // Build a Wire message targeting the current cached upstream `primary`
        let from: MacAddress = dev_mac;
        let to: MacAddress = primary;
        let td = ToDownstream::new(&from.bytes(), to, b"x");
        let msg = Message::new(from, to, PacketType::Data(Data::Downstream(td)));
        let wire: Vec<u8> = (&msg).into();
        let replies = vec![obu_lib::control::node::ReplyType::WireFlat(wire)];

        // Run handle_messages with routing handle so failover can be triggered
        let (tun, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = std::sync::Arc::new(tun);
        obu_lib::control::node::handle_messages(replies, &tun, &dev, Some(routing.clone()))
            .await
            .ok();

        // After send failure, primary should be promoted to backup
        let now_primary = routing.read().unwrap().get_cached_upstream().unwrap();
        assert_eq!(now_primary, backup);

        // Cleanup writer fd as Device owns it and will drop; explicitly close to be safe
        let _ = unsafe { libc::close(writer_fd) };
        Ok(())
    }

    #[tokio::test]
    async fn bytes_to_hex_formats() {
        let s = obu_lib::control::node::bytes_to_hex(&[0x01, 0x0a, 0xff]);
        assert_eq!(s, "01 0a ff");
    }
}
