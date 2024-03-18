mod routing;
mod session;

use super::node::ReplyType;
use crate::{
    control::{node, obu::session::Session},
    dev::Device,
    messages::{
        control::Control,
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{anyhow, Result};
use mac_address::MacAddress;
use routing::Routing;
use std::{
    sync::{Arc, RwLock},
    time::Instant,
};
use tokio_tun::Tun;

pub struct Obu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    mac: MacAddress,
    tun: Arc<Tun>,
    device: Arc<Device>,
    session: Session,
}

impl Obu {
    pub fn new(args: Args, mac: MacAddress, tun: Arc<Tun>, device: Arc<Device>) -> Result<Self> {
        let boot = Instant::now();
        let obu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args, &boot)?)),
            args,
            mac,
            tun: tun.clone(),
            device,
            session: Session::new(tun),
        };

        tracing::info!(?obu.args, "Setup Obu");
        Ok(obu)
    }

    pub async fn process(&self) {
        tokio::select! {
            m = node::wire_traffic(&self.device, |pkt, size| {
                async move {
                    let Ok(msg) = Message::try_from(&pkt[..size]) else {
                        return Ok(None);
                    };

                    let response = self.handle_msg(&msg).await;
                    tracing::trace!(incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                    response
                }
            }) => {
                if let Ok(Some(messages)) = m {
                    let _ = node::handle_messages(messages, &self.tun, &self.device).await;
                }
            },
            m = self.session.process(|x, size| {
                async move {
                    let y: &[u8] = &x[..size];
                    let Some(upstream) = self.routing.read().unwrap().get_route_to(None) else {
                        return Ok(None);
                    };

                    let outgoing = vec![ReplyType::Wire(
                        (&Message::new(
                            self.mac,
                            upstream.mac,
                            PacketType::Data(Data::Upstream(ToUpstream::new(self.mac, y))),
                        ))
                            .into(),
                    )];
                    tracing::trace!(?outgoing, "outgoing from tap");
                    Ok(Some(outgoing))
                }
            }) => {
                if let Ok(Some(messages)) = m {
                    let _ = node::handle_messages(messages, &self.tun, &self.device).await;
                }
            },
        };
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
                        self.mac,
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
                if destination == self.mac {
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
                            self.mac,
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
                .handle_heartbeat(msg, self.mac),
            PacketType::Control(Control::HeartbeatReply(_)) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(msg, self.mac),
        }
    }
}
