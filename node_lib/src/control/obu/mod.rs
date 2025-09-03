pub mod routing;
mod session;

use super::node::{BufPart, ReplyType};
use crate::{
    control::{node, obu::session::Session},
    messages::{
        control::Control,
    data::Data,
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
                        match Message::try_from(&pkt[..]) {
                            Ok(msg) => {
                                let response = match obu.handle_msg_with_backing(&msg, &pkt).await {
                                    Ok(v) => Ok(v),
                                    Err(_) => obu.handle_msg(&msg).await,
                                };
                                let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                                response
                            }
                            Err(e) => {
                                tracing::trace!(error = ?e, raw = %crate::control::node::bytes_to_hex(&pkt[..size]), "obu wire_traffic failed to parse message");
                                Ok(None)
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
                    .process(|buf_arc, _size| async move {
                        let y_arc: std::sync::Arc<[u8]> = buf_arc; // exact payload
                        let Some(upstream) = routing.read().unwrap().get_route_to(None) else {
                            return Ok(None);
                        };

                        // Zero-copy upstream: build header parts and attach the Arc payload
                        let mut parts = Vec::with_capacity(5);
                        parts.push(BufPart::Owned(upstream.mac.bytes().to_vec()));
                        parts.push(BufPart::Owned(devicec.mac_address().bytes().to_vec()));
                        parts.push(BufPart::Owned(vec![0x30, 0x30]));
                        parts.push(BufPart::Owned(vec![1u8])); // PacketType::Data
                        parts.push(BufPart::Owned(vec![0u8])); // Data::Upstream
                        parts.push(BufPart::Owned(devicec.mac_address().bytes().to_vec())); // origin
                        parts.push(BufPart::ArcSlice { data: y_arc.clone(), offset: 0, len: y_arc.len() }); // data
                        let outgoing = vec![ReplyType::WireParts(parts)];
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

    /// Zero-copy handling for wire messages using backing Arc buffer.
    async fn handle_msg_with_backing(
        &self,
        msg: &Message<'_>,
        backing: &Arc<[u8]>,
    ) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let routing = self.routing.read().unwrap();
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };
                let origin = buf.source().as_ref();
                let data = buf.data().as_ref();
                let arc_part = |sub: &[u8]| -> super::node::BufPart {
                    let base = backing.as_ptr() as usize;
                    let ptr = sub.as_ptr() as usize;
                    let len = sub.len();
                    if ptr >= base && ptr + len <= base + backing.len() {
                        super::node::BufPart::ArcSlice {
                            data: backing.clone(),
                            offset: ptr - base,
                            len,
                        }
                    } else {
                        super::node::BufPart::Owned(sub.to_vec())
                    }
                };

                let mut parts = Vec::with_capacity(7);
                parts.push(BufPart::Owned(upstream.mac.bytes().to_vec()));
                parts.push(BufPart::Owned(self.device.mac_address().bytes().to_vec()));
                parts.push(BufPart::Owned(vec![0x30, 0x30]));
                parts.push(BufPart::Owned(vec![1u8])); // PacketType::Data
                parts.push(BufPart::Owned(vec![0u8])); // Data::Upstream
                parts.push(arc_part(origin)); // origin
                parts.push(arc_part(data)); // data
                Ok(Some(vec![ReplyType::WireParts(parts)]))
            }
            PacketType::Data(Data::Downstream(buf)) => {
                let destination: [u8; 6] = buf
                    .destination()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let destination: MacAddress = destination.into();
                if destination == self.device.mac_address() {
                    let data = buf.data().as_ref();
                    // Use backing Arc part if possible
                    let base = backing.as_ptr() as usize;
                    let ptr = data.as_ptr() as usize;
                    let len = data.len();
                    let part = if ptr >= base && ptr + len <= base + backing.len() {
                        BufPart::ArcSlice { data: backing.clone(), offset: ptr - base, len }
                    } else {
                        BufPart::Owned(data.to_vec())
                    };
                    return Ok(Some(vec![ReplyType::TapParts(vec![part])]));
                }

                let target = destination;
                let routing = self.routing.read().unwrap();
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };
                let origin = buf.source().as_ref();
                let data = buf.data().as_ref();
                let arc_part = |sub: &[u8]| -> BufPart {
                    let base = backing.as_ptr() as usize;
                    let ptr = sub.as_ptr() as usize;
                    let len = sub.len();
                    if ptr >= base && ptr + len <= base + backing.len() {
                        BufPart::ArcSlice { data: backing.clone(), offset: ptr - base, len }
                    } else {
                        BufPart::Owned(sub.to_vec())
                    }
                };
                let mut parts = Vec::with_capacity(8);
                parts.push(BufPart::Owned(next_hop.mac.bytes().to_vec()));
                parts.push(BufPart::Owned(self.device.mac_address().bytes().to_vec()));
                parts.push(BufPart::Owned(vec![0x30, 0x30]));
                parts.push(BufPart::Owned(vec![1u8]));
                parts.push(BufPart::Owned(vec![1u8])); // Downstream
                parts.push(arc_part(origin));
                parts.push(BufPart::Owned(target.bytes().to_vec()));
                parts.push(arc_part(data));
                Ok(Some(vec![ReplyType::WireParts(parts)]))
            }
            PacketType::Control(_) => self.handle_msg(msg).await,
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
    use std::os::unix::io::FromRawFd;
    use std::sync::Arc;
    use tokio::io::unix::AsyncFd;

    // Helper: set fd non-blocking using libc
    fn set_nonblocking(fd: i32) {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

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

    #[tokio::test]
    async fn handle_msg_with_backing_upstream_uses_arc_slice() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        // minimal Args and routing
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Obu, hello_history: 2, hello_periodicity: None },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
        ));

        // Build dummy Tun and Device
        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        // pipe for device fd
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        set_nonblocking(reader_fd);

        let our_mac: MacAddress = [9u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            our_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap(),
        ));

        let session = Arc::new(super::session::Session::new(tun.clone()));
        let obu = super::Obu {
            args,
            routing: routing.clone(),
            tun: tun.clone(),
            device: device.clone(),
            session,
        };

        // Populate routing with a heartbeat and select upstream
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
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

        // Build a raw upstream data frame and parse it from its own backing Arc
        let from: MacAddress = [3u8; 6].into();
        let to: MacAddress = [4u8; 6].into();
        let payload = b"hello-up";
        let tu = ToUpstream::new(from, payload);
        let msg_struct = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = Message::try_from(&arc[..]).expect("parse msg");

        let out = obu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");

        assert_eq!(out.len(), 1);
        match &out[0] {
            super::ReplyType::WireParts(parts) => {
                // Expect at least one ArcSlice matching payload length
                assert!(parts.iter().any(|p| matches!(p, super::BufPart::ArcSlice { len, .. } if *len == payload.len())));
            }
            other => panic!("expected WireParts, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_msg_with_backing_downstream_to_self_tap_parts() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Obu, hello_history: 2, hello_periodicity: None },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
        ));

        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        set_nonblocking(reader_fd);

        let our_mac: MacAddress = [11u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            our_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap(),
        ));

        let session = Arc::new(super::session::Session::new(tun.clone()));
        let obu = super::Obu {
            args,
            routing: routing.clone(),
            tun: tun.clone(),
            device: device.clone(),
            session,
        };

        // Build downstream addressed to self
        let src = [4u8; 6];
        let payload = b"down-self";
        let td = ToDownstream::new(&src, our_mac, payload);
        let msg_struct = Message::new(our_mac, [255u8; 6].into(), PacketType::Data(Data::Downstream(td)));
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = Message::try_from(&arc[..]).expect("parse msg");

        let out = obu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");

        assert_eq!(out.len(), 1);
        match &out[0] {
            super::ReplyType::TapParts(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(parts[0], super::BufPart::ArcSlice { len, .. } if len == payload.len()))
            }
            other => panic!("expected TapParts, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_msg_with_backing_downstream_forward_wire_parts() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Obu, hello_history: 2, hello_periodicity: None },
        };
        let boot = std::time::Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args, &boot).expect("routing"),
        ));

        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        set_nonblocking(reader_fd);

        let our_mac: MacAddress = [12u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            our_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap(),
        ));

        let session = Arc::new(super::session::Session::new(tun.clone()));
        let obu = super::Obu {
            args,
            routing: routing.clone(),
            tun: tun.clone(),
            device: device.clone(),
            session,
        };

        // Populate routing
        let hb_source: MacAddress = [77u8; 6].into();
        let pkt_from: MacAddress = [78u8; 6].into();
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

        // Build downstream addressed to other (hb_source)
        let src = [5u8; 6];
        let target_mac: MacAddress = hb_source;
        let payload = b"down-other";
        let td = ToDownstream::new(&src, target_mac, payload);
        let msg_struct = Message::new(target_mac, [255u8; 6].into(), PacketType::Data(Data::Downstream(td)));
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = Message::try_from(&arc[..]).expect("parse msg");

        let out = obu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");

        assert_eq!(out.len(), 1);
        match &out[0] {
            super::ReplyType::WireParts(parts) => {
                // Origin and data should be ArcSlices
                let arc_slices = parts
                    .iter()
                    .filter(|p| matches!(p, super::BufPart::ArcSlice { .. }))
                    .count();
                assert!(arc_slices >= 2);
            }
            other => panic!("expected WireParts, got {other:?}"),
        }
    }
}
