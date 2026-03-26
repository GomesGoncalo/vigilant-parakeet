#[cfg(test)]
mod forwarding_tests {
    use anyhow::Result;
    use mac_address::MacAddress;
    use node_lib::messages::{
        control::heartbeat::Heartbeat,
        control::heartbeat::HeartbeatReply,
        control::Control,
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };

    #[tokio::test]
    async fn upstream_data_always_forwarded_to_server() -> Result<()> {
        use rsu_lib::control::{handle_msg_for_test, routing::Routing, ClientCache};

        // RSU no longer decrypts - it forwards all upstream data opaquely.
        // Even "bad ciphertext" should be forwarded to the server.
        let args = rsu_lib::RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 1,
                server_ip: None,
                server_port: 8080,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(Routing::new(&args)?));
        let cache = std::sync::Arc::new(ClientCache::default());

        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&[2u8; 6]); // dest
        inner.extend_from_slice(&from.bytes()); // source
        inner.extend_from_slice(&[0u8; 4]);
        let tu = ToUpstream::new(from, &inner);
        let msg = Message::new(from, [2u8; 6].into(), PacketType::Data(Data::Upstream(tu)));
        let out = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg)?;
        // RSU forwards everything to server - should return Some with WireFlat
        assert!(out.is_some());
        let replies = out.unwrap();
        assert!(replies
            .iter()
            .any(|r| matches!(r, rsu_lib::control::node::ReplyType::WireFlat(_))));
        Ok(())
    }

    #[tokio::test]
    async fn upstream_unicast_forwards_with_correct_source() -> Result<()> {
        use rsu_lib::control::{handle_msg_for_test, routing::Routing, ClientCache};

        let args = rsu_lib::RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000,
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(Routing::new(&args)?));
        let cache = std::sync::Arc::new(ClientCache::default());

        // Seed a route via heartbeat reply
        {
            let mut w = routing.write().unwrap();
            let _ = w.send_heartbeat([9u8; 6].into());
        }
        let target_node: MacAddress = [77u8; 6].into();
        let next_hop: MacAddress = [88u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(0), 0, [9u8; 6].into());
        let hbr = HeartbeatReply::from_sender(&hb, target_node);
        let reply = Message::new(
            next_hop,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr)),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat_reply(&reply, [9u8; 6].into())?;

        let dest_client: MacAddress = [10u8; 6].into();
        cache.store_mac(dest_client, target_node);

        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_client.bytes());
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(&[0u8; 4]);
        let tu = ToUpstream::new(from, &inner);
        let msg = Message::new(from, dest_client, PacketType::Data(Data::Upstream(tu)));
        let out = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg)?;
        assert!(out.is_some());
        let replies = out.unwrap();
        // Should produce a WireFlat (UpstreamForward to server)
        assert_eq!(replies.len(), 1);
        if let rsu_lib::control::node::ReplyType::WireFlat(bytes) = &replies[0] {
            let parsed =
                server_lib::UpstreamForward::try_from_bytes(bytes).expect("valid upstream forward");
            assert_eq!(parsed.rsu_mac, MacAddress::from([9u8; 6]));
            assert_eq!(parsed.obu_source_mac, from);
        } else {
            panic!("expected WireFlat");
        }
        Ok(())
    }
}
