pub mod args;
pub use args::{ObuArgs, ObuParameters};

mod routing;
mod session;

use routing::Routing;
use session::Session;

use node_lib::{
    control::node::ReplyType,
    messages::{
        control::Control,
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    },
};
use anyhow::{anyhow, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use mac_address::MacAddress;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
};
use std::any::Any;
use tokio::time::Instant;

pub struct Obu {
    args: ObuArgs,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    session: Arc<Session>,
}

pub trait Node: Send + Sync {
    /// For runtime downcasting to concrete node types.
    fn as_any(&self) -> &dyn Any;
}

impl Node for Obu {
    fn as_any(&self) -> &dyn Any {
        self
    }
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
        self.routing.read().unwrap().get_cached_upstream()
    }

    /// Return the cached upstream Route if present (hops, mac, latency).
    pub fn cached_upstream_route(&self) -> Option<node_lib::control::route::Route> {
        // routing.get_route_to(None) returns Option<Route>
        self.routing.read().unwrap().get_route_to(None)
    }

    /// Return the number of cached upstream candidates kept for failover.
    pub fn cached_upstream_candidates_len(&self) -> usize {
        self.routing
            .read()
            .unwrap()
            .get_cached_candidates()
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn wire_traffic_task(obu: Arc<Self>) -> Result<()> {
        let device = obu.device.clone();
        let tun = obu.tun.clone();
        let routing_handle = obu.routing.clone();
        tokio::task::spawn(async move {
            loop {
                let obu_c = obu.clone();
                let messages = node_lib::control::node::wire_traffic(&device, |pkt, size| {
                    let obu = obu_c.clone();
                    async move {
                        // Try to parse multiple messages from the packet
                        let data = &pkt[..size];
                        let mut all_responses = Vec::new();
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    let response = obu.handle_msg(&msg).await;
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

                // upstream packets can promote the next candidate.
                if let Ok(Some(messages)) = messages {
                    // Send the messages like RSU does
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

    fn session_task(&self) -> Result<()> {
        let tun = self.tun.clone();
        let device = self.device.clone();
        let routing_for_handle = self.routing.clone();

        tokio::task::spawn(async move {
            loop {
                // Simple tap traffic handling without callback for now
                // Can be enhanced to match RSU pattern later
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        let packet = msg.get_packet_type();
        let from = msg.from()?;

        match packet {
            PacketType::Data(Data::Upstream(to_upstream)) => {
                tracing::trace!(?msg, "obu handling Upstream");
                // Simple inline handling - can be enhanced later
                Ok(None)
            }
            PacketType::Control(Control::Heartbeat(hb)) => {
                tracing::trace!(?msg, "obu handling Heartbeat");
                let handle_result = {
                    let mut routing = self.routing.write().unwrap();
                    routing.handle_heartbeat(msg, from)
                };
                match handle_result {
                    Ok(reply_opt) => Ok(reply_opt),
                    Err(e) => {
                        tracing::warn!(?e, "failed to handle heartbeat");
                        Err(e)
                    }
                }
            }
            PacketType::Control(Control::HeartbeatReply(reply)) => {
                let routing_result = {
                    let mut routing = self.routing.write().unwrap();
                    routing.handle_heartbeat_reply(msg, from)
                };
                match routing_result {
                    Ok(reply_opt) => Ok(reply_opt),
                    Err(e) => {
                        tracing::warn!(?e, "failed to handle heartbeat reply");
                        Err(e)
                    }
                }
            }
            PacketType::Data(Data::Downstream(_)) => {
                tracing::trace!(?msg, "obu handling Downstream");
                Ok(None)
            }
        }
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: ObuArgs) -> Result<Arc<dyn Node>> {
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

    let device = Arc::new(Device::new(&args.bind)?);

    Ok(Obu::new(args, tun, device)?)
}

pub fn create_with_vdev(
    args: ObuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    Ok(Obu::new(args, tun, node_device)?)
}