use super::{
    node::{Node, ReplyType, Route},
    Args,
};
use crate::{
    dev::Device,
    messages::{ControlType, HeartBeatReply, Message, PacketType},
};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tracing::Level;

#[derive(Debug)]
struct RoutingTarget {
    hops: u32,
    mac: MacAddress,
    latency: Option<Duration>,
}

#[derive(Debug)]
struct Routing {
    args: Args,
    boot: Instant,
    routes: HashMap<
        MacAddress,
        IndexMap<
            u32,
            (
                Duration,
                MacAddress,
                u32,
                HashMap<MacAddress, Vec<RoutingTarget>>,
            ),
        >,
    >,
    cached_upstream: Arc<Mutex<Option<MacAddress>>>,
}

impl Routing {
    fn new(args: &Args, boot: &Instant) -> Result<Self> {
        if args.node_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            args: args.clone(),
            boot: *boot,
            routes: HashMap::default(),
            cached_upstream: Arc::new(Mutex::new(None)),
        })
    }

    fn handle_heartbeat(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let Ok(PacketType::Control(ControlType::HeartBeat(message))) = pkt.next_layer() else {
            bail!("this is supposed to be a HeartBeat");
        };

        let old_route = self.get_route_to(message.source);
        let from: [u8; 6] = pkt.from().try_into()?;
        let from_mac: MacAddress = from.into();
        let old_route_from = self.get_route_to(from_mac);
        if self.routes.get(&message.source).is_some() {
            return Ok(None);
        }

        let entry = self
            .routes
            .entry(message.source)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        if let Some((_, _, hops, _)) = entry.get(&message.id) {
            if hops <= &message.hops {
                return Ok(None);
            }
        }

        entry.insert(
            message.id,
            (
                Instant::now().duration_since(self.boot),
                from.into(),
                message.hops,
                HashMap::default(),
            ),
        );

        let entry_from = self
            .routes
            .entry(from_mac)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry_from.first().is_some_and(|(x, _)| x > &message.id) {
            entry_from.clear();
        }

        if entry_from.len() == entry_from.capacity() && entry_from.capacity() > 0 {
            entry_from.swap_remove_index(0);
        }

        entry_from.insert(
            message.id,
            (
                Instant::now().duration_since(self.boot),
                from.into(),
                1,
                HashMap::default(),
            ),
        );

        match (old_route, self.get_route_to(message.source)) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %message.source,
                    through = %new_route,
                    "route created on heartbeat",
                );
            }
            (_, None) => (),
            (Some(old_route), Some(new_route)) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %message.source,
                        through = %new_route,
                        was_through = %old_route,
                        "route changed on heartbeat",
                    );
                }
            }
        }

        if message.source != from_mac {
            match (old_route_from, self.get_route_to(from_mac)) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %from_mac,
                        through = %new_route,
                        "route created on heartbeat",
                    );
                }
                (_, None) => (),
                (Some(old_route_from), Some(new_route)) => {
                    if old_route_from.mac != new_route.mac {
                        tracing::event!(
                            Level::DEBUG,
                            from = %mac,
                            to = %from_mac,
                            through = %new_route,
                            was_through = %old_route_from,
                            "route changed on heartbeat",
                        );
                    }
                }
            }
        }

        Ok(Some(vec![
            ReplyType::Wire(
                Message::new(
                    mac.bytes(),
                    [255; 6],
                    &PacketType::Control(ControlType::HeartBeat(message.clone())),
                )
                .into(),
            ),
            ReplyType::Wire(
                Message::new(
                    mac.bytes(),
                    from,
                    &PacketType::Control(ControlType::HeartBeatReply(HeartBeatReply::from_sender(
                        &message, mac,
                    ))),
                )
                .into(),
            ),
        ]))
    }

    fn handle_heartbeat_reply(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let Ok(PacketType::Control(ControlType::HeartBeatReply(message))) = pkt.next_layer() else {
            bail!("this is supposed to be a HeartBeat Reply");
        };

        let old_route = self.get_route_to(message.sender);
        let from: [u8; 6] = pkt.from().try_into()?;
        let from_mac: MacAddress = from.into();
        let old_route_from = self.get_route_to(from_mac);
        let Some(source_entries) = self.routes.get_mut(&message.source) else {
            bail!("we don't know how to reach that source");
        };

        let Some((duration, next_upstream, _, downstream)) = source_entries.get_mut(&message.id)
        else {
            bail!("no recollection of the next hop for this route");
        };

        let latency = Instant::now().duration_since(self.boot) - *duration;
        match downstream.entry(message.sender) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(RoutingTarget {
                    hops: message.hops,
                    mac: from_mac,
                    latency: Some(latency),
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![RoutingTarget {
                    hops: message.hops,
                    mac: from_mac,
                    latency: Some(latency),
                }]);
            }
        };

        match downstream.entry(from_mac) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(RoutingTarget {
                    hops: 1,
                    mac: from_mac,
                    latency: None,
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![RoutingTarget {
                    hops: 1,
                    mac: from_mac,
                    latency: None,
                }]);
            }
        };

        let sender = message.sender;
        let reply = Ok(Some(vec![ReplyType::Wire(
            Message::new(
                mac.bytes(),
                next_upstream.bytes(),
                &PacketType::Control(ControlType::HeartBeatReply(message)),
            )
            .into(),
        )]));

        match (old_route, self.get_route_to(sender)) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %sender,
                    through = %new_route,
                    "route created on heartbeat reply",
                );
            }
            (_, None) => (),
            (Some(old_route), Some(new_route)) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %sender,
                        through = %new_route,
                        was_through = %old_route,
                        "route changed on heartbeat reply",
                    );
                }
            }
        }

        if sender != from_mac {
            match (old_route_from, self.get_route_to(from_mac)) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %from_mac,
                        through = %new_route,
                        "route created on heartbeat reply",
                    );
                }
                (_, None) => (),
                (Some(old_route_from), Some(new_route)) => {
                    if old_route_from.mac != new_route.mac {
                        tracing::event!(
                            Level::DEBUG,
                            from = %mac,
                            to = %from_mac,
                            through = %new_route,
                            was_through = %old_route_from,
                            "route changed on heartbeat reply",
                        );
                    }
                }
            }
        }

        reply
    }

    fn get_route_to(&self, mac: MacAddress) -> Option<Route> {
        let mut upstream_routes: Vec<_> = self
            .routes
            .iter()
            .flat_map(|(rsu_mac, seqs)| {
                seqs.iter()
                    .map(move |(seq, (_, mac, hops, _))| (seq, rsu_mac, mac, hops))
            })
            .filter(|(_, rsu_mac, _, _)| rsu_mac == &&mac)
            .collect();
        upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));

        let mut cached = self.cached_upstream.lock().unwrap();
        if let Some(cached_upstream) = cached.as_ref() {
            if let Some((_, _, upstream_route, hops)) = upstream_routes
                .iter()
                .find(|(_, rsu_mac, _, _)| *rsu_mac == cached_upstream)
            {
                return Some(Route {
                    hops: **hops,
                    mac: **upstream_route,
                    latency: None,
                });
            }
        } else if let Some((_, _, upstream_route, hops)) = upstream_routes.first() {
            *cached = Some(**upstream_route);
            return Some(Route {
                hops: **hops,
                mac: **upstream_route,
                latency: None,
            });
        }
        std::mem::drop(cached);

        let route_options: IndexMap<_, _> = self
            .routes
            .iter()
            .flat_map(|(rsus, im)| {
                im.iter()
                    .map(move |(seq, (dur, mac, hops, rout))| (seq, (dur, mac, hops, rout, rsus)))
            })
            .collect();

        let route_options = route_options
            .iter()
            .rev()
            .flat_map(|(seq, (_, _, _, m, _))| {
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
                            latency.push(route.latency.map(|x| x.as_micros()));
                            *e += 1;
                        })
                        .or_insert((
                            1,
                            *seq,
                            vec![route.mac],
                            vec![route.latency.map(|x| x.as_micros())],
                        ));
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
                        if let Some(val) = *val {
                            let val = val as f32;

                            if entry.0 > val {
                                entry.0 = val;
                            }

                            if entry.2 < val {
                                entry.2 = val;
                            }

                            entry.1 += val;
                            entry.3 += 1.0;
                        }
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

pub struct Obu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    mac: MacAddress,
}

impl Obu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let boot = Instant::now();
        let obu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args, &boot)?)),
            args,
            mac,
        };

        tracing::info!(?obu.args, %obu.mac, "Setup Obu");
        Ok(obu)
    }
}

impl Node for Obu {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(buf)) => Ok(Some(vec![ReplyType::Tap(vec![buf.into()])])),
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat(msg, self.mac),
            Ok(PacketType::Control(ControlType::HeartBeatReply(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(msg, self.mac),
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }

    fn generate(&self, _dev: Arc<Device>) {}

    fn get_route_to(&self, mac: MacAddress) -> Option<MacAddress> {
        self.routing
            .read()
            .unwrap()
            .get_route_to(mac)
            .map(|x| x.mac)
    }
}
