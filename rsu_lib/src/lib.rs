pub mod args;
pub use args::{RsuArgs, RsuParameters};

mod routing;
use routing::Routing;

use node_lib::{
    control::{client_cache::ClientCache, node::ReplyType},
    messages::{
        control::Control,
        data::{Data, ToDownstream},
        message::Message,
        packet_type::PacketType,
    },
};
use anyhow::{anyhow, bail, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use itertools::Itertools;
use mac_address::MacAddress;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
    time::Duration,
};
use std::any::Any;

pub struct Rsu {
    args: RsuArgs,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: Arc<ClientCache>,
}

pub trait Node: Send + Sync {
    /// For runtime downcasting to concrete node types.
    fn as_any(&self) -> &dyn Any;
}

impl Node for Rsu {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Rsu {
    pub fn new(args: RsuArgs, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let rsu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            tun,
            device,
            cache: ClientCache::default().into(),
        });

        tracing::info!(?rsu.args, "Setup Rsu");
        rsu.hello_task()?;
        rsu.process_tap_traffic()?;
        Self::wire_traffic_task(rsu.clone())?;
        Ok(rsu)
    }

    /// Get route to a specific MAC address. Used for testing latency measurement.
    pub fn get_route_to(&self, mac: MacAddress) -> Option<node_lib::control::route::Route> {
        self.routing.read().unwrap().get_route_to(Some(mac))
    }

    /// Get count of next hops in routing table. Used for testing.
    pub fn next_hop_count(&self) -> usize {
        self.routing.read().unwrap().iter_next_hops().count()
    }

    fn wire_traffic_task(rsu: Arc<Self>) -> Result<()> {
        let device = rsu.device.clone();
        let tun = rsu.tun.clone();

        tokio::task::spawn(async move {
            loop {
                let rsu = rsu.clone();
                let messages = node_lib::control::node::wire_traffic(&device, |pkt, size| {
                    async move {
                        // Try to parse multiple messages from the packet
                        let data = &pkt[..size];
                        let mut all_responses = Vec::new();
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    tracing::trace!(offset = offset, parsed = ?msg, "rsu wire_traffic parsed message");
                                    let response = rsu.handle_msg(&msg).await;
                                    let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                    tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node_lib::control::node::get_msgs(&response), "transaction");

                                    if let Ok(Some(responses)) = response {
                                        all_responses.extend(responses);
                                    }

                                    // Calculate message size to advance offset
                                    let msg_bytes: Vec<Vec<u8>> = (&msg).into();
                                    let msg_size: usize = msg_bytes.iter().map(|chunk| chunk.len()).sum();
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
                })
                .await;

                // downstream packets from tun.
                if let Ok(Some(messages)) = messages {
                    // Send the messages using handle_messages-like logic
                    for msg in messages {
                        match msg {
                            ReplyType::Tap(data_vec) => {
                                for data in data_vec {
                                    let _ = tun.send_all(&data).await;
                                }
                            }
                            ReplyType::Wire(wire_vec) => {
                                let vec: Vec<IoSlice> = wire_vec.iter().map(|x| IoSlice::new(x)).collect();
                                let _ = device.send_vectored(&vec).await;
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    fn hello_task(&self) -> Result<()> {
        let periodicity = self.args.rsu_params.hello_periodicity;
        let periodicity = Duration::from_millis(periodicity.into());
        let routing = self.routing.clone();
        let device = self.device.clone();

        tokio::task::spawn(async move {
            loop {
                let msg: Vec<Vec<u8>> = {
                    let mut routing = routing.write().unwrap();
                    let msg = routing.send_heartbeat(device.mac_address());
                    tracing::trace!(?msg, "generated hello");
                    (&msg).into()
                };
                // Flatten and log the generated bytes as hex so we can compare
                // what the RSU writes vs what the OBU reads in wire_traffic.
                let flat: Vec<u8> = msg.iter().flat_map(|x| x.iter()).copied().collect();
                tracing::trace!(n = flat.len(), raw = %node_lib::control::node::bytes_to_hex(&flat), "rsu generated raw");
                let vec: Vec<IoSlice> = msg.iter().map(|x| IoSlice::new(x)).collect();
                let _ = device
                    .send_vectored(&vec)
                    .await
                    .inspect_err(|e| tracing::error!(?e, "error sending hello"));
                let _ = tokio_timerfd::sleep(periodicity).await;
            }
        });
        Ok(())
    }

    fn process_tap_traffic(&self) -> Result<()> {
        let tun = self.tun.clone();
        let device = self.device.clone();
        let cache = self.cache.clone();
        let routing = self.routing.clone();
        let enable_encryption = self.args.rsu_params.enable_encryption;
        tokio::task::spawn(async move {
            loop {
                let devicec = device.clone();
                let cache = cache.clone();
                let routing = routing.clone();
                let result = node_lib::control::node::tap_traffic(&tun, |pkt, size| async move {
                    let data: &[u8] = &pkt[..size];
                    let to: [u8; 6] = data[0..6].try_into()?;
                    let to: MacAddress = to.into();
                    let target = cache.get(to);
                    let from: [u8; 6] = data[6..12].try_into()?;
                    let from: MacAddress = from.into();
                    let source_mac = devicec.mac_address().bytes();
                    cache.store_mac(from, devicec.mac_address());

                    let routing = routing.read().unwrap();
                    // Simple downstream forwarding - can be enhanced later
                    if let Some(target_mac) = target {
                        if let Some(route) = routing.get_route_to(Some(target_mac)) {
                            let downstream_msg = node_lib::messages::message::Message::new(
                                devicec.mac_address(),
                                route.mac,
                                node_lib::messages::packet_type::PacketType::Data(
                                    node_lib::messages::data::Data::Downstream(
                                        node_lib::messages::data::ToDownstream::new(
                                            &source_mac,
                                            target_mac,
                                            data,
                                        ),
                                    ),
                                ),
                            );
                            return Ok(Some(vec![ReplyType::Wire((&downstream_msg).into())]));
                        }
                    }
                    Ok(None)
                }).await;

                if let Ok(Some(messages)) = result {
                    // Send the wire messages
                    for message in messages {
                        if let ReplyType::Wire(wire_msgs) = message {
                            let vec: Vec<IoSlice> = wire_msgs.iter().map(|x| IoSlice::new(x)).collect();
                            let _ = device.send_vectored(&vec).await;
                        }
                    }
                }
            }
        });
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        let packet = msg.get_packet_type();
        let from = msg.from()?;

        match packet {
            PacketType::Data(Data::Upstream(to_upstream)) => {
                tracing::trace!(?msg, "rsu handling ToUpstream");
                
                // Decrypt the entire frame if encryption is enabled
                let decrypted_payload = if self.args.rsu_params.enable_encryption {
                    match node_lib::crypto::decrypt_payload(to_upstream.data()) {
                        Ok(decrypted_data) => decrypted_data,
                        Err(_) => return Ok(None),
                    }
                } else {
                    to_upstream.data().to_vec()
                };

                // Extract MAC addresses from decrypted data
                let to: [u8; 6] = decrypted_payload
                    .get(0..6)
                    .ok_or_else(|| anyhow!("decrypted frame too short for destination MAC"))?
                    .try_into()?;
                let to: MacAddress = to.into();
                let from_inner: [u8; 6] = decrypted_payload
                    .get(6..12)
                    .ok_or_else(|| anyhow!("decrypted frame too short for source MAC"))?
                    .try_into()?;
                let from_inner: MacAddress = from_inner.into();
                let source: [u8; 6] = to_upstream
                    .source()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("message source too short"))?
                    .try_into()?;
                let source: MacAddress = source.into();
                self.cache.store_mac(from_inner, source);
                let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
                let mut target = self.cache.get(to);
                let mut messages = Vec::with_capacity(1);

                if bcast_or_mcast || target.is_some_and(|x| x == self.device.mac_address()) {
                    messages.push(ReplyType::Tap(vec![decrypted_payload.clone()]));
                    target = None;
                }

                let routing = self.routing.read().unwrap();
                messages.extend(if bcast_or_mcast {
                    routing
                        .iter_next_hops()
                        .filter(|x| x != &&source)
                        .filter_map(|x| {
                            let route = routing.get_route_to(Some(*x))?;
                            Some((*x, route.mac))
                        })
                        .filter_map(|(_target, next_hop)| {
                            // For broadcast traffic, encrypt if needed
                            let downstream_data = if self.args.rsu_params.enable_encryption {
                                match node_lib::crypto::encrypt_payload(&decrypted_payload) {
                                    Ok(encrypted) => encrypted,
                                    Err(_) => return None,
                                }
                            } else {
                                decrypted_payload.clone()
                            };
                            let downstream_msg = Message::new(
                                self.device.mac_address(),
                                next_hop,
                                PacketType::Data(Data::Downstream(
                                    node_lib::messages::data::ToDownstream::new(
                                        to_upstream.source(),
                                        _target,
                                        &downstream_data,
                                    ),
                                )),
                            );
                            Some(ReplyType::Wire((&downstream_msg).into()))
                        })
                        .collect::<Vec<_>>()
                } else if let Some(target_mac) = target {
                    if let Some(route) = routing.get_route_to(Some(target_mac)) {
                        let downstream_data = if self.args.rsu_params.enable_encryption {
                            match node_lib::crypto::encrypt_payload(&decrypted_payload) {
                                Ok(encrypted) => encrypted,
                                Err(_) => return Ok(Some(messages)),
                            }
                        } else {
                            decrypted_payload
                        };
                        let downstream_msg = Message::new(
                            self.device.mac_address(),
                            route.mac,
                            PacketType::Data(Data::Downstream(
                                node_lib::messages::data::ToDownstream::new(
                                    to_upstream.source(),
                                    target_mac,
                                    &downstream_data,
                                ),
                            )),
                        );
                        vec![ReplyType::Wire((&downstream_msg).into())]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                });

                Ok(Some(messages))
            }
            PacketType::Control(Control::HeartbeatReply(reply)) => {
                self.routing
                    .write()
                    .unwrap()
                    .handle_heartbeat_reply(msg, from)
            }
            PacketType::Data(Data::Downstream(_)) | PacketType::Control(Control::Heartbeat(_)) => {
                Ok(None)
            }
        }
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: RsuArgs) -> Result<Arc<dyn Node>> {
    // Use the real tokio_tun builder type in non-test builds.
    use tokio_tun::Tun as RealTokioTun;

    let real_tun: RealTokioTun = if args.ip.is_some() {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap()
            .mtu(args.mtu)
            .up()
            .address(args.ip.unwrap())
            .build()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
    } else {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap()
            .up()
            .build()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
    };

    let tun = Arc::new(Tun::new(real_tun));

    let device = Arc::new(Device::new(&args.bind)?);  // Changed from bind_to_interface to new

    Ok(Rsu::new(args, tun, device)?)
}

pub fn create_with_vdev(
    args: RsuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    Ok(Rsu::new(args, tun, node_device)?)
}