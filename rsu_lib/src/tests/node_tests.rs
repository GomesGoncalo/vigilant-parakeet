#[cfg(test)]
mod node_tests {
    use anyhow::Result;
    use mac_address::MacAddress;
    use node_lib::messages::{
        control::heartbeat::{Heartbeat, HeartbeatReply},
        control::Control,
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };

    #[tokio::test]
    async fn handle_msg_heartbeat_reply_only_for_self_source() -> Result<()> {
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

        // Build a HeartbeatReply where hbr.source != device_mac => expect None
        let src: MacAddress = [1u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(0), 0u32, src);
        let reply_sender: MacAddress = [2u8; 6].into();
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let msg = Message::new(
            [3u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr)),
        );
        let out = handle_msg_for_test(routing.clone(), [9u8; 6].into(), cache.clone(), &msg)?;
        assert!(out.is_none());

        // Now same but with device_mac as source => expect Some
        let hb_self = Heartbeat::new(std::time::Duration::from_millis(0), 0u32, [9u8; 6].into());
        let hbr2 = HeartbeatReply::from_sender(&hb_self, reply_sender);
        let msg2 = Message::new(
            [4u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr2)),
        );
        let out2 = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg2)?;
        assert!(out2.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn upstream_broadcast_forwards_to_server() -> Result<()> {
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

        // Build upstream broadcast payload: dest ff:ff.., source client
        let client: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&[255u8; 6]);
        inner.extend_from_slice(&client.bytes());
        inner.extend_from_slice(&[0u8; 2]);
        let tu = ToUpstream::new(client, &inner);
        let msg = Message::new(client, [255u8; 6].into(), PacketType::Data(Data::Upstream(tu)));

        let out = handle_msg_for_test(routing.clone(), [9u8; 6].into(), cache.clone(), &msg)?;
        assert!(out.is_some());
        let v = out.unwrap();
        // RSU forwards upstream data to server as WireFlat (UpstreamForward)
        assert!(!v.is_empty());
        assert!(v
            .iter()
            .any(|x| matches!(x, rsu_lib::control::node::ReplyType::WireFlat(_))));
        Ok(())
    }
}
