pub mod dh_key_store;
pub mod node;
pub mod routing;
mod routing_cache;

// Re-export shared modules from node_lib to avoid duplication
pub use node_lib::control::route;
pub use node_lib::control::routing_utils;
mod session;

use crate::args::ObuArgs;
use anyhow::{anyhow, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use mac_address::MacAddress;
use node::ReplyType;
use node_lib::messages::{
    auth::Auth, control::Control, data::Data, message::Message, packet_type::PacketType,
};
use routing::Routing;
use session::CryptoState;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::Instrument;

// Re-export type aliases for cleaner code
use node_lib::{Shared, SharedDevice, SharedTun};

pub struct Obu {
    args: ObuArgs,
    routing: Shared<Routing>,
    tun: SharedTun,
    device: SharedDevice,
    node_name: String,
    crypto: Arc<CryptoState>,
}

impl Obu {
    pub fn new(
        args: ObuArgs,
        tun: Arc<Tun>,
        device: Arc<Device>,
        node_name: String,
    ) -> Result<Arc<Self>> {
        let _span = tracing::info_span!("node", name = %node_name).entered();

        let boot = Instant::now();
        let routing = Arc::new(std::sync::RwLock::new(Routing::new(&args, &boot)?));
        let crypto = Arc::new(CryptoState::new(&args)?);

        let obu = Arc::new(Self {
            routing,
            tun,
            device,
            node_name,
            crypto,
            args,
        });

        tracing::info!(
            bind = %obu.args.bind,
            mac = %obu.device.mac_address(),
            mtu = obu.args.mtu,
            hello_history = obu.args.obu_params.hello_history,
            cached_candidates = obu.args.obu_params.cached_candidates,
            "OBU initialized"
        );
        if !obu.args.obu_params.enable_encryption {
            tracing::warn!(
                "Encryption is DISABLED — all traffic is sent in the clear. \
                 Set --enable-encryption to protect data payloads."
            );
        }
        if !obu.args.obu_params.enable_dh_signatures {
            tracing::warn!(
                "DH signatures are DISABLED — key exchange messages are not authenticated. \
                 Set --enable-dh-signatures to prevent MITM key substitution."
            );
        }
        CryptoState::start_session_task(
            obu.crypto.clone(),
            obu.tun.clone(),
            obu.device.clone(),
            obu.routing.clone(),
            obu.node_name.clone(),
        )?;
        Obu::wire_traffic_task(obu.clone())?;
        if obu.args.obu_params.enable_encryption {
            CryptoState::start_dh_rekey_task(
                obu.crypto.clone(),
                obu.device.clone(),
                obu.routing.clone(),
                obu.node_name.clone(),
            )?;
        }
        Ok(obu)
    }

    /// Attach a live RSSI table for proximity-based RSU selection.
    ///
    /// The table maps neighbor MAC addresses to their received signal strength in dBm.
    /// It is populated by the simulator fading task (from computed distances) or by a
    /// real radio driver.  Once attached, `select_and_cache_upstream` uses RSSI as the
    /// primary RSU selection metric with a 3 dB hysteresis.
    pub fn set_rssi_table(&self, table: routing::RssiTable) {
        self.routing
            .write()
            .expect("routing table write lock poisoned")
            .set_rssi_table(table);
    }

    /// Return the cached upstream MAC if present.
    pub fn cached_upstream_mac(&self) -> Option<mac_address::MacAddress> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_upstream()
    }

    /// Check whether a DH session with the server has been established.
    pub fn has_dh_session(&self) -> bool {
        self.crypto.has_dh_session()
    }

    /// Return the cached upstream Route if present (hops, mac, latency).
    pub fn cached_upstream_route(&self) -> Option<route::Route> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_route_to(None)
    }

    /// Return the number of cached upstream candidates kept for failover.
    pub fn cached_upstream_candidates_len(&self) -> usize {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_candidates()
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Return this node's name.
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Return this node's VANET MAC address.
    pub fn mac_address(&self) -> MacAddress {
        self.device.mac_address()
    }

    /// Return the ordered list of cached upstream candidates (primary first).
    pub fn get_upstream_candidates(&self) -> Vec<MacAddress> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_candidates()
            .unwrap_or_default()
    }

    /// Return the established DH session info: `(key_id, age_secs)`.
    /// Returns `None` when no session has been established yet.
    pub fn get_dh_session_info(&self) -> Option<(u32, u64)> {
        self.crypto.get_dh_session_info()
    }

    /// Return `(key_id, handshake_duration_ms, age_ms, dh_group, signing_algo)` for
    /// the current server session, if established.
    pub fn get_session_timing(&self) -> Option<(u32, u64, u64, &'static str, &'static str)> {
        self.crypto.get_session_timing()
    }

    /// Immediately trigger a DH re-key exchange, bypassing the normal interval.
    pub fn trigger_rekey(&self) {
        self.crypto.trigger_rekey();
    }

    /// Return whether a pending DH exchange is in progress.
    pub fn has_dh_pending(&self) -> bool {
        self.crypto.has_dh_pending()
    }

    fn wire_traffic_task(obu: Arc<Self>) -> Result<()> {
        let device = obu.device.clone();
        let tun = obu.tun.clone();
        let routing_handle = obu.routing.clone();
        let node_name = obu.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let obu_c = obu.clone();
                    let messages = node::wire_traffic(&device, |pkt, size| {
                        let obu = obu_c.clone();
                        async move {
                            let data = &pkt[..size];
                            let mut all_responses = Vec::with_capacity(4);
                            let mut offset = 0;

                            while offset < data.len() {
                                match Message::try_from(&data[offset..]) {
                                    Ok(msg) => {
                                        let response = obu.handle_msg(&msg).await;

                                        if let Ok(Some(responses)) = response {
                                            all_responses.extend(responses);
                                        }
                                        offset += msg.wire_size();
                                    }
                                    Err(e) => {
                                        tracing::trace!(
                                            offset = offset,
                                            remaining = data.len() - offset,
                                            total_size = data.len(),
                                            error = %e,
                                            "Failed to parse message, stopping batch processing"
                                        );
                                        break;
                                    }
                                }
                            }

                            if all_responses.is_empty() {
                                Ok(None)
                            } else {
                                Ok(Some(all_responses))
                            }
                        }
                    })
                    .await;
                    if let Ok(Some(messages)) = messages {
                        let _ = node::handle_messages_batched(
                            messages,
                            &tun,
                            &device,
                            Some(routing_handle.clone()),
                        )
                        .await;
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let routing = self
                    .routing
                    .read()
                    .expect("routing table read lock poisoned during upstream data handling");
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                let mut wire = Vec::with_capacity(24 + buf.data().len());
                Message::serialize_upstream_forward_into(
                    buf,
                    self.device.mac_address(),
                    upstream.mac,
                    &mut wire,
                );
                Ok(Some(vec![ReplyType::WireFlat(wire)]))
            }
            PacketType::Data(Data::Downstream(buf)) => {
                let destination: [u8; 6] = buf
                    .destination()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let destination: MacAddress = destination.into();
                let is_for_us =
                    destination == self.device.mac_address() || destination.bytes()[0] & 0x1 != 0;
                if is_for_us {
                    let Some(payload_data) = self.crypto.decrypt_downstream(buf.data()) else {
                        return Ok(None);
                    };
                    return Ok(Some(vec![ReplyType::TapFlat(payload_data)]));
                }
                let target = destination;
                let routing = self
                    .routing
                    .read()
                    .expect("routing table read lock poisoned during downstream forwarding");
                Ok(Some({
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    let mut wire = Vec::with_capacity(30 + buf.data().len());
                    Message::serialize_downstream_forward_into(
                        buf,
                        self.device.mac_address(),
                        next_hop.mac,
                        &mut wire,
                    );
                    vec![ReplyType::WireFlat(wire)]
                }))
            }
            PacketType::Control(Control::Heartbeat(_)) => self
                .routing
                .write()
                .expect("routing table write lock poisoned during heartbeat handling")
                .handle_heartbeat(msg, self.device.mac_address()),
            PacketType::Control(Control::HeartbeatReply(_)) => self
                .routing
                .write()
                .expect("routing table write lock poisoned during heartbeat reply handling")
                .handle_heartbeat_reply(msg, self.device.mac_address()),
            PacketType::Auth(Auth::KeyExchangeInit(ke_init)) => self
                .crypto
                .handle_key_exchange_init(ke_init, msg, self.device.mac_address(), &self.routing),
            PacketType::Auth(Auth::KeyExchangeReply(ke_reply)) => self
                .crypto
                .handle_key_exchange_reply(ke_reply, msg, self.device.mac_address(), &self.routing),
            PacketType::Auth(Auth::SessionTerminated(st)) => {
                self.crypto
                    .handle_session_terminated(st, self.device.mac_address(), &self.routing)
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: Shared<Routing>,
    device_mac: mac_address::MacAddress,
    msg: &node_lib::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use node_lib::messages::{auth::Auth, control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(buf)) => {
            let routing = routing
                .read()
                .expect("routing table read lock poisoned in test helper");
            let Some(upstream) = routing.get_route_to(None) else {
                return Ok(None);
            };

            let wire: Vec<u8> = (&node_lib::messages::message::Message::new(
                device_mac,
                upstream.mac,
                PacketType::Data(Data::Upstream(buf.clone())),
            ))
                .into();
            Ok(Some(vec![ReplyType::WireFlat(wire)]))
        }
        PacketType::Data(Data::Downstream(buf)) => {
            let destination: [u8; 6] = buf
                .destination()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let destination: mac_address::MacAddress = destination.into();
            if destination == device_mac {
                return Ok(Some(vec![ReplyType::TapFlat(buf.data().to_vec())]));
            }

            let target = destination;
            let routing = routing
                .read()
                .expect("routing table read lock poisoned in test helper");
            Ok(Some({
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };

                let wire: Vec<u8> = (&node_lib::messages::message::Message::new(
                    device_mac,
                    next_hop.mac,
                    PacketType::Data(Data::Downstream(buf.clone())),
                ))
                    .into();
                vec![ReplyType::WireFlat(wire)]
            }))
        }
        PacketType::Control(Control::Heartbeat(_)) => routing
            .write()
            .expect("routing table write lock poisoned in test helper")
            .handle_heartbeat(msg, device_mac),
        PacketType::Control(Control::HeartbeatReply(_)) => routing
            .write()
            .expect("routing table write lock poisoned in test helper")
            .handle_heartbeat_reply(msg, device_mac),
        PacketType::Auth(Auth::KeyExchangeInit(_))
        | PacketType::Auth(Auth::KeyExchangeReply(_))
        | PacketType::Auth(Auth::SessionTerminated(_)) => {
            // Key exchange and session messages not handled in basic test helper
            Ok(None)
        }
    }
}

#[cfg(test)]
mod obu_tests {
    use super::{handle_msg_for_test, routing::Routing};
    use mac_address::MacAddress;
    use node_lib::messages::{
        control::heartbeat::Heartbeat,
        control::Control,
        data::{Data, ToDownstream, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };
    use tokio::time::Instant;

    #[test]
    fn upstream_with_no_cached_upstream_returns_none() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();
        let payload = b"p";
        let tu = ToUpstream::new(from, payload);
        let msg = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));

        let res = handle_msg_for_test(routing, [9u8; 6].into(), &msg).expect("ok");
        assert!(res.is_none());
    }

    #[test]
    fn downstream_to_self_returns_tap() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let src = [4u8; 6];
        let dest_mac: MacAddress = [10u8; 6].into();
        let payload = b"abc";
        let td = ToDownstream::new(&src, dest_mac, payload);
        let msg = Message::new(
            dest_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing, dest_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert_eq!(v.len(), 1);
        match &v[0] {
            super::ReplyType::TapFlat(_) => {}
            _ => panic!("expected TapFlat"),
        }
    }

    #[test]
    fn upstream_with_cached_upstream_returns_wire() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        let from: MacAddress = [3u8; 6].into();
        let to: MacAddress = [4u8; 6].into();
        let payload = b"x";
        let tu = ToUpstream::new(from, payload);
        let msg = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_generates_forward_and_reply() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [11u8; 6].into();
        let pkt_from: MacAddress = [12u8; 6].into();
        let our_mac: MacAddress = [13u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 2u32, hb_source);
        let msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.len() >= 2);
        assert!(v.iter().all(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_reply_updates_routing_and_replies() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [21u8; 6].into();
        let pkt_from: MacAddress = [22u8; 6].into();
        let our_mac: MacAddress = [23u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 7u32, hb_source);
        let initial = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&initial, our_mac)
            .expect("handled");

        let reply_sender: MacAddress = [42u8; 6].into();
        let hbr =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [55u8; 6].into();
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &reply_msg).expect("ok");
        assert!(res.is_some());
        let out = res.unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn downstream_to_other_forwards_wire_when_route_exists() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [77u8; 6].into();
        let pkt_from: MacAddress = [78u8; 6].into();
        let our_mac: MacAddress = [79u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        let src = [3u8; 6];
        let target_mac: MacAddress = hb_source;
        let payload = b"ok";
        let td = ToDownstream::new(&src, target_mac, payload);
        let msg = Message::new(
            target_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn downstream_to_other_returns_none_when_no_route() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let our_mac: MacAddress = [90u8; 6].into();
        let target_mac: MacAddress = [91u8; 6].into();
        let src = [5u8; 6];
        let payload = b"nope";
        let td = ToDownstream::new(&src, target_mac, payload);
        let msg = Message::new(
            target_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_none());
    }
}
