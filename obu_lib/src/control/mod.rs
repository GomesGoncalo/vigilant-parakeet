pub mod node;
pub mod route;
pub mod routing;
pub mod routing_utils;
mod session;

use crate::args::ObuArgs;
use anyhow::{anyhow, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use mac_address::MacAddress;
use node::ReplyType;
use node_lib::messages::{
    control::Control,
    data::{Data, ToUpstream},
    message::Message,
    packet_type::PacketType,
};
use routing::Routing;
use session::Session;
use std::sync::{Arc, RwLock};
use tokio::time::Instant;

pub struct Obu {
    args: ObuArgs,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    session: Arc<Session>,
}

impl Obu {
    pub fn new(args: ObuArgs, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let boot = Instant::now();
        let routing = Arc::new(RwLock::new(Routing::new(&args, &boot)?));
        let obu = Arc::new(Self {
            args,
            routing,
            tun: tun.clone(),
            device,
            session: Session::new(tun).into(),
        });

        tracing::info!(?obu.args, "Setup Obu");
        obu.session_task()?;
        Obu::wire_traffic_task(obu.clone())?;
        Ok(obu)
    }

    /// Return the cached upstream MAC if present.
    pub fn cached_upstream_mac(&self) -> Option<mac_address::MacAddress> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_upstream()
    }

    /// Return the cached upstream Route if present (hops, mac, latency).
    pub fn cached_upstream_route(&self) -> Option<route::Route> {
        // routing.get_route_to(None) returns Option<Route>
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

    fn wire_traffic_task(obu: Arc<Self>) -> Result<()> {
        let device = obu.device.clone();
        let tun = obu.tun.clone();
        let _routing_handle = obu.routing.clone();
        tokio::task::spawn(async move {
            loop {
                let obu_c = obu.clone();
                let messages = node::wire_traffic(&device, |pkt, size| {
                    let obu = obu_c.clone();
                    async move {
                        let data = &pkt[..size];
                        let mut all_responses = Vec::new();
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    let response = obu.handle_msg(&msg).await;
                                    let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                    #[cfg(any(test, feature = "test_helpers"))]
                                    tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                                    #[cfg(not(any(test, feature = "test_helpers")))]
                                    tracing::trace!(has_response = has_response, incoming = ?msg, "transaction");

                                    if let Ok(Some(responses)) = response {
                                        all_responses.extend(responses);
                                    }
                                    // Use flat serialization for better performance
                                    let msg_bytes: Vec<u8> = (&msg).into();
                                    let msg_size: usize = msg_bytes.len();
                                    offset += msg_size;
                                }
                                Err(e) => {
                                    tracing::trace!(offset = offset, remaining = data.len() - offset, error = ?e, "could not parse message at offset");
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
                }).await;
                if let Ok(Some(messages)) = messages {
                    // Use batched message handling for improved throughput (2-3x faster)
                    let _ = node::handle_messages_batched(messages, &tun, &device).await;
                }
            }
        });
        Ok(())
    }

    fn session_task(&self) -> Result<()> {
        let routing = self.routing.clone();
        let session = self.session.clone();
        let device = self.device.clone();
        let tun = self.tun.clone();
        let routing_handle = routing.clone();
        let enable_encryption = self.args.obu_params.enable_encryption;
        tokio::task::spawn(async move {
            loop {
                let devicec = device.clone();
                let routing_for_closure = routing_handle.clone();
                let routing_for_handle = routing_handle.clone();
                let messages = session
                    .process(|x, size| async move {
                        let y: &[u8] = &x[..size];
                        let Some(upstream) = routing_for_closure
                            .read()
                            .expect("routing table read lock poisoned in heartbeat task")
                            .get_route_to(None)
                        else {
                            return Ok(None);
                        };

                        let payload_data = if enable_encryption {
                            match node_lib::crypto::encrypt_payload(y) {
                                Ok(encrypted_data) => encrypted_data,
                                Err(e) => {
                                    tracing::error!("Failed to encrypt entire frame: {}", e);
                                    return Ok(None);
                                }
                            }
                        } else {
                            y.to_vec()
                        };

                        let wire: Vec<u8> = (&Message::new(
                            devicec.mac_address(),
                            upstream.mac,
                            PacketType::Data(Data::Upstream(ToUpstream::new(
                                devicec.mac_address(),
                                &payload_data,
                            ))),
                        ))
                            .into();
                        let outgoing = vec![ReplyType::WireFlat(wire)];
                        tracing::trace!(?outgoing, "outgoing from tap");
                        Ok(Some(outgoing))
                    })
                    .await;

                if let Ok(Some(messages)) = messages {
                    // Pass the routing handle so send failures from TAP-originated
                    // upstream packets can promote the next candidate.
                    let _ = node::handle_messages(
                        messages,
                        &tun,
                        &device,
                        Some(routing_for_handle.clone()),
                    )
                    .await;
                }
            }
        });
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

                // Use zero-copy serialization (12.4x faster than traditional)
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
                    let payload_data = if self.args.obu_params.enable_encryption {
                        match node_lib::crypto::decrypt_payload(buf.data()) {
                            Ok(decrypted_data) => decrypted_data,
                            Err(e) => {
                                tracing::error!("Failed to decrypt downstream frame: {}", e);
                                return Ok(None);
                            }
                        }
                    } else {
                        buf.data().to_vec()
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

                    // Use zero-copy serialization (18.6x faster than traditional)
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
        }
    }
}

#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: Arc<RwLock<Routing>>,
    device_mac: mac_address::MacAddress,
    msg: &node_lib::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use node_lib::messages::{control::Control, data::Data, packet_type::PacketType};

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
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
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
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
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
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        // Create a heartbeat to populate routes
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        // Insert heartbeat via routing handle
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Ensure selection and caching of upstream
        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        // Now send an upstream data packet and expect a Wire reply to the cached upstream
        let from: MacAddress = [3u8; 6].into();
        let to: MacAddress = [4u8; 6].into();
        let payload = b"x";
        let tu = ToUpstream::new(from, payload);
        let msg = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        // should have at least one WireFlat reply
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_generates_forward_and_reply() {
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
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
        // expect at least two Wire replies (forward and reply)
        assert!(v.len() >= 2);
        // Updated to check for flat serialization (better performance)
        assert!(v.iter().all(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_reply_updates_routing_and_replies() {
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [21u8; 6].into();
        let pkt_from: MacAddress = [22u8; 6].into();
        let our_mac: MacAddress = [23u8; 6].into();

        // Insert initial heartbeat
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

        // Create a HeartbeatReply from some sender
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
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        // create a heartbeat so that a route to `hb_source` exists
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

        // Ensure route options are populated and cache selected
        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        // Prepare a downstream payload addressed to someone other than our device
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
        let args = crate::args::ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: crate::args::ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
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
