use crate::{
    control::route::Route,
    messages::{
        control::{heartbeat::HeartbeatReply, Control},
        message::Message,
        packet_type::PacketType,
    },
    Args, ReplyType,
};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Option<Duration>,
}

#[derive(Debug)]
pub struct Routing {
    args: Args,
    boot: Instant,
    routes: HashMap<
        MacAddress,
        IndexMap<u32, (Duration, MacAddress, u32, HashMap<MacAddress, Vec<Target>>)>,
    >,
    cached_upstream: Arc<Mutex<Option<MacAddress>>>,
}

impl Routing {
    pub fn new(args: &Args, boot: &Instant) -> Result<Self> {
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

    pub fn handle_heartbeat(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::Heartbeat(message)) = pkt.get_packet_type() else {
            bail!("this is supposed to be a HeartBeat");
        };

        let old_route = self.get_route_to(Some(message.source()));
        let old_route_from = self.get_route_to(Some(pkt.from()?));
        let entry = self
            .routes
            .entry(message.source())
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id()) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        if let Some((_, _, hops, _)) = entry.get(&message.id()) {
            if hops < &message.hops() {
                return Ok(None);
            }
        }

        entry.insert(
            message.id(),
            (
                Instant::now().duration_since(self.boot),
                pkt.from()?,
                message.hops(),
                HashMap::default(),
            ),
        );

        let entry_from = self
            .routes
            .entry(pkt.from()?)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry_from.first().is_some_and(|(x, _)| x > &message.id()) {
            entry_from.clear();
        }

        if entry_from.len() == entry_from.capacity() && entry_from.capacity() > 0 {
            entry_from.swap_remove_index(0);
        }

        entry_from.insert(
            message.id(),
            (
                Instant::now().duration_since(self.boot),
                pkt.from()?,
                1,
                HashMap::default(),
            ),
        );

        match (old_route, self.get_route_to(Some(message.source()))) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %message.source(),
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
                        to = %message.source(),
                        through = %new_route,
                        was_through = %old_route,
                        "route changed on heartbeat",
                    );
                }
            }
        }

        if message.source() != pkt.from()? {
            match (old_route_from, self.get_route_to(Some(pkt.from()?))) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %pkt.from()?,
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
                            to = %pkt.from()?,
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
                (&Message::new(
                    mac,
                    [255; 6].into(),
                    PacketType::Control(Control::Heartbeat(message.clone())),
                ))
                    .into(),
            ),
            ReplyType::Wire(
                (&Message::new(
                    mac,
                    pkt.from()?,
                    PacketType::Control(Control::HeartbeatReply(HeartbeatReply::from_sender(
                        message, mac,
                    ))),
                ))
                    .into(),
            ),
        ]))
    }

    pub fn handle_heartbeat_reply(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::HeartbeatReply(message)) = pkt.get_packet_type() else {
            bail!("this is supposed to be a HeartBeat Reply");
        };

        let old_route = self.get_route_to(Some(message.sender()));
        let old_route_from = self.get_route_to(Some(pkt.from()?));
        let Some(source_entries) = self.routes.get_mut(&message.source()) else {
            bail!("we don't know how to reach that source");
        };

        let Some((duration, next_upstream, _, downstream)) = source_entries.get_mut(&message.id())
        else {
            bail!("no recollection of the next hop for this route");
        };

        let latency = Instant::now().duration_since(self.boot) - *duration;
        match downstream.entry(message.sender()) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(Target {
                    hops: message.hops(),
                    mac: pkt.from()?,
                    latency: Some(latency),
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![Target {
                    hops: message.hops(),
                    mac: pkt.from()?,
                    latency: Some(latency),
                }]);
            }
        };

        match downstream.entry(pkt.from()?) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(Target {
                    hops: 1,
                    mac: pkt.from()?,
                    latency: None,
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![Target {
                    hops: 1,
                    mac: pkt.from()?,
                    latency: None,
                }]);
            }
        };

        let sender = message.sender();
        let reply = Ok(Some(vec![ReplyType::Wire(
            (&Message::new(
                mac,
                *next_upstream,
                PacketType::Control(Control::HeartbeatReply(message.clone())),
            ))
                .into(),
        )]));

        match (old_route, self.get_route_to(Some(sender))) {
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

        if sender != pkt.from()? {
            match (old_route_from, self.get_route_to(Some(pkt.from()?))) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %pkt.from()?,
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
                            to = %pkt.from()?,
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

    pub fn get_route_to(&self, mac: Option<MacAddress>) -> Option<Route> {
        let Some(mac) = mac else {
            return self.cached_upstream.lock().unwrap().map(|mac| Route {
                hops: 1,
                mac,
                latency: None,
            });
        };
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
        }

        if let Some((_, _, upstream_route, hops)) = upstream_routes.first() {
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
