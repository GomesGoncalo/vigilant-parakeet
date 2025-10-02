#[cfg(test)]
mod encryption_tests {
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
    async fn upstream_with_encryption_and_bad_ciphertext_is_ignored() -> Result<()> {
        use rsu_lib::control::{handle_msg_for_test, routing::Routing, ClientCache};

        let args = rsu_lib::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 1,
                enable_encryption: true,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(Routing::new(&args)?));
        let cache = std::sync::Arc::new(ClientCache::default());

        let from: MacAddress = [1u8; 6].into();
        // Build bogus ciphertext: RSU expects encrypted payload yet we pass plaintext frame
        let mut inner = Vec::new();
        inner.extend_from_slice(&[2u8; 6]); // dest
        inner.extend_from_slice(&from.bytes()); // source
        inner.extend_from_slice(&[0u8; 4]);
        let tu = ToUpstream::new(from, &inner);
        let msg = Message::new(from, [2u8; 6].into(), PacketType::Data(Data::Upstream(tu)));
        let out = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg)?;
        // RSU should try to decrypt and fail, returning None
        assert!(out.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn broadcast_with_encryption_fans_out_and_taps() -> Result<()> {
        use rsu_lib::control::{handle_msg_for_test, routing::Routing, ClientCache};

        let args = rsu_lib::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000,
                cached_candidates: 3,
                enable_encryption: true,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(Routing::new(&args)?));
        let cache = std::sync::Arc::new(ClientCache::default());

        // Seed routes with two next hops via heartbeat replies
        {
            let mut w = routing.write().unwrap();
            let _ = w.send_heartbeat([9u8; 6].into());
        }
        let hb = Heartbeat::new(std::time::Duration::from_millis(0), 0, [9u8; 6].into());
        for nh in [[10u8; 6], [11u8; 6]] {
            let hbr = HeartbeatReply::from_sender(&hb, nh.into());
            let reply = Message::new(
                nh.into(),
                [255u8; 6].into(),
                PacketType::Control(Control::HeartbeatReply(hbr)),
            );
            let _ = routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(&reply, [9u8; 6].into())?;
        }

        // Build plaintext frame, RSU will encrypt when faning out.
        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&[255u8; 6]); // broadcast
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(&[0u8; 6]);
        let tu = ToUpstream::new(from, &inner);
        let msg = Message::new(from, [255u8; 6].into(), PacketType::Data(Data::Upstream(tu)));

        let out = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg)?;
        assert!(out.is_some());
        let replies = out.unwrap();
        // Expect at least one TapFlat and one WireFlat
        assert!(replies.iter().any(|r| matches!(r, rsu_lib::control::node::ReplyType::TapFlat(_))));
        assert!(replies.iter().any(|r| matches!(r, rsu_lib::control::node::ReplyType::WireFlat(_))));
        Ok()
    }

    #[tokio::test]
    async fn unicast_with_encryption_forwards() -> Result<()> {
        use rsu_lib::control::{handle_msg_for_test, routing::Routing, ClientCache};

        let args = rsu_lib::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: rsu_lib::RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000,
                cached_candidates: 3,
                enable_encryption: true,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(Routing::new(&args)?));
        let cache = std::sync::Arc::new(ClientCache::default());

        // Seed a route for a target node via heartbeat reply
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

        // Cache a client mapping to target_node
        let dest_client: MacAddress = [10u8; 6].into();
        cache.store_mac(dest_client, target_node);

        // Build plaintext upstream unicast; RSU will encrypt before forwarding.
        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_client.bytes());
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(&[0u8; 4]);
        let tu = ToUpstream::new(from, &inner);
        let msg = Message::new(from, dest_client, PacketType::Data(Data::Upstream(tu)));
        let out = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg)?;
        assert!(out.is_some());
        // Expect a single WireFlat reply
        assert!(out
            .unwrap()
            .iter()
            .any(|r| matches!(r, rsu_lib::control::node::ReplyType::WireFlat(_))));
        Ok(())
    }
}
