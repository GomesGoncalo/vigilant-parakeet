use super::{
    node::{Node, ReplyType, Route},
    Args,
};
use crate::{
    dev::{Device, OutgoingMessage},
    messages::{ControlType, HeartBeat, Message, PacketType},
};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tracing::{Instrument, Level};

#[derive(Debug)]
struct RoutingTarget {
    hops: u32,
    mac: MacAddress,
    latency: Duration,
}

#[derive(Debug)]
struct Routing {
    hb_seq: u32,
    boot: Instant,
    sent: IndexMap<u32, (Duration, HashMap<MacAddress, Vec<RoutingTarget>>)>,
}

impl Routing {
    fn new(args: &Args) -> Result<Self> {
        if args.node_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            hb_seq: 0,
            boot: Instant::now(),
            sent: IndexMap::with_capacity(usize::try_from(args.node_params.hello_history)?),
        })
    }

    fn send_heartbeat(&mut self, address: MacAddress) -> Message {
        let message = HeartBeat::new(
            address,
            Instant::now().duration_since(self.boot),
            self.hb_seq,
        );

        if self.sent.first().is_some_and(|(x, _)| x > &message.id) {
            self.sent.clear();
        }

        if self.sent.len() == self.sent.capacity() && self.sent.capacity() > 0 {
            self.sent.swap_remove_index(0);
        }

        let _ = self
            .sent
            .insert(message.id, (message.now, HashMap::default()));

        self.hb_seq += 1;

        Message::new(
            address.bytes(),
            [255; 6],
            &PacketType::Control(ControlType::HeartBeat(message)),
        )
    }

    fn handle_heartbeat_reply(&mut self, msg: &Message, address: MacAddress) -> Result<()> {
        let Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) = msg.next_layer() else {
            bail!("only heartbeat reply messages accepted");
        };

        let old_route = self.get_route_to(hbr.sender);
        let Some((_, map)) = self.sent.get_mut(&hbr.id) else {
            tracing::warn!("outdated heartbeat");
            return Ok(());
        };

        let from: [u8; 6] = msg.from().try_into()?;
        let latency = Instant::now().duration_since(self.boot) - hbr.now;
        match map.entry(hbr.sender) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(RoutingTarget {
                    hops: hbr.hops,
                    mac: from.into(),
                    latency,
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![RoutingTarget {
                    hops: hbr.hops,
                    mac: from.into(),
                    latency,
                }]);
            }
        };

        match (old_route, self.get_route_to(hbr.sender)) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %address,
                    to = %hbr.sender,
                    through = %new_route,
                    "route created",
                );
            }
            (_, None) => (),
            (Some(old_route), Some(new_route)) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %address,
                        to = %hbr.sender,
                        through = %new_route,
                        was_through = %old_route,
                        "route changed",
                    );
                }
            }
        }
        Ok(())
    }

    fn get_route_to(&self, mac: MacAddress) -> Option<Route> {
        let route_options = self
            .sent
            .iter()
            .rev()
            .flat_map(|(seq, (_, m))| {
                let seq = *seq;
                m.iter().map(move |(mac, route)| (seq, mac, route))
            })
            .filter(|(_, smac, _)| &&mac == smac)
            .flat_map(|(seq, mac, route)| route.iter().map(move |r| (seq, mac, r)))
            .fold(
                IndexMap::default(),
                |mut hm: IndexMap<u32, (usize, u32, Vec<_>, Vec<_>)>, (seq, _, route)| {
                    hm.entry(route.hops)
                        .and_modify(|(e, _, next, latency)| {
                            next.push(route.mac);
                            latency.push(route.latency.as_micros());
                            *e += 1;
                        })
                        .or_insert((1, seq, vec![route.mac], vec![route.latency.as_micros()]));
                    hm
                },
            );

        let (min_hops, _) = route_options.first()?;

        let route_options: IndexMap<_, _> = route_options
            .iter()
            .filter(|(h, _)| h == &min_hops)
            .flat_map(|(hops, (_count, _min_seq, next, latency))| {
                latency.iter().zip(next).fold(
                    HashMap::default(),
                    |mut hm: HashMap<MacAddress, (f32, f32, f32, f32, u32)>, (val, mac)| {
                        let entry = hm
                            .entry(*mac)
                            .or_insert((f32::MAX, 0.0, f32::MIN, 0.0, *hops));
                        let val = *val as f32;

                        if entry.0 > val {
                            entry.0 = val;
                        }

                        if entry.2 < val {
                            entry.2 = val;
                        }

                        entry.1 += val;
                        entry.3 += 1.0;
                        hm
                    },
                )
            })
            .map(|(mac, (min, sum, _, n, hops))| {
                (((min + (sum / n)) / 2.0) as usize, (mac, hops, sum / n))
            })
            .collect();

        route_options
            .first()
            .map(|(_, (mac, hops, latency))| Route {
                hops: *hops,
                mac: *mac,
                latency: Some(Duration::from_micros(*latency as u64)),
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        control::{
            args::{NodeParameters, NodeType},
            rsu::Routing,
            Args,
        },
        messages::{ControlType, PacketType},
    };

    #[test]
    fn can_generate_heartbeat() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 1,
                hello_periodicity: None,
            },
        };

        let Ok(mut routing) = Routing::new(&args) else {
            panic!("did not build a routing object");
        };
        let message = routing.send_heartbeat([1; 6].into());

        assert_eq!(message.from(), &[1; 6]);
        assert_eq!(message.to(), &[255; 6]);

        let PacketType::Control(ControlType::HeartBeat(message)) =
            message.next_layer().expect("contains a next layer")
        else {
            panic!("did not generate a heartbeat");
        };

        assert_eq!(message.source, [1; 6].into());
        assert_eq!(message.hops, 1);
        assert_eq!(routing.hb_seq, 1);
    }

    #[test]
    fn keeps_track_of_n_heartbeats_and_cannot_build_without_keeping_history() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 0,
                hello_periodicity: None,
            },
        };

        Routing::new(&args).expect_err("should be an error");

        for i in 1..10 {
            let args = Args {
                bind: String::default(),
                tap_name: None,
                ip: None,
                node_params: NodeParameters {
                    node_type: NodeType::Rsu,
                    hello_history: i,
                    hello_periodicity: None,
                },
            };

            let mut routing = Routing::new(&args).expect("should be an error");
            assert_eq!(
                routing.sent.capacity(),
                usize::try_from(i).expect("could not convert capacity")
            );

            (1..=i * 2).for_each(|j| {
                let msg = routing.send_heartbeat([1; 6].into());

                assert!(routing.sent.len() <= routing.sent.capacity());
                assert!(
                    routing.sent.len()
                        == std::cmp::min(
                            usize::try_from(j).expect("can convert"),
                            routing.sent.capacity()
                        )
                );
                assert_eq!(
                    routing.sent.capacity(),
                    usize::try_from(i).expect("could not convert capacity")
                );
                let h = routing.sent.last().expect("must have a last");
                let hb = match msg.next_layer().expect("must have next_layer") {
                    PacketType::Control(ControlType::HeartBeat(hb)) => hb,
                    _ => panic!("built the wrong message"),
                };
                assert_eq!(h.0, &hb.id);
            });
        }
    }
}

pub struct Rsu {
    args: Args,
    mac: MacAddress,
    routing: Arc<RwLock<Routing>>,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            mac,
        };

        tracing::info!(?rsu.args, %rsu.mac, "Setup Rsu");
        Ok(rsu)
    }
}

impl Node for Rsu {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(buf)) => Ok(Some(vec![ReplyType::Tap(vec![buf.into()])])),
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => Ok(None),
            Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) => {
                if hbr.source == self.mac {
                    let span =
                        tracing::debug_span!(target: "hello", "hello task", rsu.mac=%self.mac);
                    let _g = span.enter();
                    self.routing
                        .write()
                        .unwrap()
                        .handle_heartbeat_reply(msg, self.mac)?;
                }

                Ok(None)
            }
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }

    fn generate(&self, dev: Arc<Device>) {
        let mac = self.mac;
        let routing = self.routing.clone();
        let hello_periodicity = self.args.node_params.hello_periodicity;
        let span = tracing::error_span!(target: "hello", "hello task", rsu.mac=%mac);
        let _g = span.enter();
        if let Some(hello_periodicity) = hello_periodicity {
            tokio::spawn(
                async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(hello_periodicity.into())).await;
                        let message = routing.write().unwrap().send_heartbeat(mac);
                        tracing::trace!(target: "pkt", ?message, "pkt");
                        match dev.tx.send(OutgoingMessage::Vectored(message.into())).await {
                            Ok(()) => tracing::trace!("sent hello"),
                            Err(e) => tracing::error!(?e, "error sending hello"),
                        };
                    }
                }
                .in_current_span(),
            );
        } else {
            tracing::error!(?self.args, "Rsu configured without hello_periodicity parameter");
        }
    }

    fn get_route_to(&self, mac: MacAddress) -> Option<MacAddress> {
        self.routing
            .read()
            .unwrap()
            .get_route_to(mac)
            .map(|x| x.mac)
    }
}
