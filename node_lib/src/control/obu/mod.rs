pub mod routing;
mod session;

use super::node::ReplyType;
use crate::{
    control::{node, obu::session::Session},
    messages::{
        control::Control,
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{anyhow, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use mac_address::MacAddress;
use routing::Routing;
use std::{
    sync::{Arc, RwLock},
    time::Instant,
};

pub struct Obu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    session: Arc<Session>,
}

impl Obu {
    pub fn new(args: Args, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let boot = Instant::now();
        let obu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args, &boot)?)),
            args,
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
        self.routing.read().unwrap().get_cached_upstream()
    }

    /// Return the cached upstream Route if present (hops, mac, latency).
    pub fn cached_upstream_route(&self) -> Option<crate::control::route::Route> {
        // routing.get_route_to(None) returns Option<Route>
        self.routing.read().unwrap().get_route_to(None)
    }

    fn wire_traffic_task(obu: Arc<Self>) -> Result<()> {
        let device = obu.device.clone();
        let tun = obu.tun.clone();
        tokio::task::spawn(async move {
            loop {
                let obu = obu.clone();
                let messages = node::wire_traffic(&device, |pkt, size| {
                    async move {
                        match Message::try_from(&pkt[..size]) {
                            Ok(msg) => {
                                let response = obu.handle_msg(&msg).await;
                                let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                                response
                            }
                            Err(e) => {
                                tracing::trace!(error = ?e, raw = %crate::control::node::bytes_to_hex(&pkt[..size]), "obu wire_traffic failed to parse message");
                                return Ok(None);
                            }
                        }
                    }
                }).await;
                if let Ok(Some(messages)) = messages {
                    let _ = node::handle_messages(messages, &tun, &device).await;
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
        tokio::task::spawn(async move {
            loop {
                let devicec = device.clone();
                let routing = routing.clone();
                let messages = session
                    .process(|x, size| async move {
                        let y: &[u8] = &x[..size];
                        let Some(upstream) = routing.read().unwrap().get_route_to(None) else {
                            return Ok(None);
                        };

                        let outgoing = vec![ReplyType::Wire(
                            (&Message::new(
                                devicec.mac_address(),
                                upstream.mac,
                                PacketType::Data(Data::Upstream(ToUpstream::new(
                                    devicec.mac_address(),
                                    y,
                                ))),
                            ))
                                .into(),
                        )];
                        tracing::trace!(?outgoing, "outgoing from tap");
                        Ok(Some(outgoing))
                    })
                    .await;

                if let Ok(Some(messages)) = messages {
                    let _ = node::handle_messages(messages, &tun, &device).await;
                }
            }
        });
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let routing = self.routing.read().unwrap();
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    (&Message::new(
                        self.device.mac_address(),
                        upstream.mac,
                        PacketType::Data(Data::Upstream(buf.clone())),
                    ))
                        .into(),
                )]))
            }
            PacketType::Data(Data::Downstream(buf)) => {
                let destination: [u8; 6] = buf
                    .destination()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let destination: MacAddress = destination.into();
                if destination == self.device.mac_address() {
                    return Ok(Some(vec![ReplyType::Tap(vec![buf.data().to_vec()])]));
                }

                let target = destination;
                let routing = self.routing.read().unwrap();
                Ok(Some({
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    vec![ReplyType::Wire(
                        (&Message::new(
                            self.device.mac_address(),
                            next_hop.mac,
                            PacketType::Data(Data::Downstream(buf.clone())),
                        ))
                            .into(),
                    )]
                }))
            }
            PacketType::Control(Control::Heartbeat(_)) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat(msg, self.device.mac_address()),
            PacketType::Control(Control::HeartbeatReply(_)) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(msg, self.device.mac_address()),
        }
    }
}

// Test-only helper that mirrors `handle_msg` logic but operates on supplied
// routing and device_mac so tests can exercise message handling without
// constructing a full `Obu` (which requires tun and device runtime setup).
#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: std::sync::Arc<std::sync::RwLock<Routing>>,
    device_mac: mac_address::MacAddress,
    msg: &crate::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use crate::messages::{control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(buf)) => {
            let routing = routing.read().unwrap();
            let Some(upstream) = routing.get_route_to(None) else {
                return Ok(None);
            };

            Ok(Some(vec![ReplyType::Wire(
                (&crate::messages::message::Message::new(
                    device_mac,
                    upstream.mac,
                    PacketType::Data(Data::Upstream(buf.clone())),
                ))
                    .into(),
            )]))
        }
        PacketType::Data(Data::Downstream(buf)) => {
            let destination: [u8; 6] = buf
                .destination()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let destination: mac_address::MacAddress = destination.into();
            if destination == device_mac {
                return Ok(Some(vec![ReplyType::Tap(vec![buf.data().to_vec()])]));
            }

            let target = destination;
            let routing = routing.read().unwrap();
            Ok(Some({
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };

                vec![ReplyType::Wire(
                    (&crate::messages::message::Message::new(
                        device_mac,
                        next_hop.mac,
                        PacketType::Data(Data::Downstream(buf.clone())),
                    ))
                        .into(),
                )]
            }))
        }
        PacketType::Control(Control::Heartbeat(_)) => {
            routing.write().unwrap().handle_heartbeat(msg, device_mac)
        }
        PacketType::Control(Control::HeartbeatReply(_)) => routing
            .write()
            .unwrap()
            .handle_heartbeat_reply(msg, device_mac),
    }
}

#[cfg(test)]
mod obu_tests {
    use super::handle_msg_for_test;
    use crate::args::{NodeParameters, NodeType};
    use crate::messages::{
        control::heartbeat::Heartbeat,
        control::Control,
        data::{Data, ToDownstream, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };
    use crate::Args;
    use mac_address::MacAddress;

    #[test]
    fn upstream_with_no_cached_upstream_returns_none() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
            super::ReplyType::Tap(bufs) => {
                assert_eq!(bufs.len(), 1);
            }
            _ => panic!("expected Tap"),
        }
    }

    #[test]
    fn upstream_with_cached_upstream_returns_wire() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
        // should have at least one Wire reply
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::Wire(_))));
    }

    #[test]
    fn heartbeat_generates_forward_and_reply() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
        assert!(v.iter().all(|x| matches!(x, super::ReplyType::Wire(_))));
    }

    #[test]
    fn heartbeat_reply_updates_routing_and_replies() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
            crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
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
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::Wire(_))));
    }

    #[test]
    fn downstream_to_other_returns_none_when_no_route() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
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
