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
use arc_swap::ArcSwapOption;
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::{
    collections::{hash_map::Entry, HashMap},
    time::{Duration, Instant},
};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::Routing;
    use crate::{
        args::{NodeParameters, NodeType},
        messages::{
            control::heartbeat::Heartbeat,
            control::heartbeat::HeartbeatReply,
            control::Control,
            message::Message,
            packet_type::PacketType,
        },
        Args, ReplyType,
    };
    use mac_address::MacAddress;
    use std::time::{Duration, Instant};

    #[test]
    fn handle_heartbeat_creates_route_and_returns_replies() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [2u8; 6].into();
        let pkt_from: MacAddress = [3u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(Duration::from_millis(1), 1u32, hb_source);
        let msg = Message::new(pkt_from, [255u8; 6].into(), PacketType::Control(Control::Heartbeat(hb.clone())));

        let res = routing.handle_heartbeat(&msg, our_mac).expect("handled");
        assert!(res.is_some());
        let vec = res.unwrap();
        // Should produce two wire replies (heartbeat forward and reply)
        assert!(vec.len() >= 2);

        // Now we should be able to get a route to hb_source
        let route = routing.get_route_to(Some(hb_source));
        assert!(route.is_some());
    }

    #[test]
    fn handle_heartbeat_reply_updates_downstream_and_replies() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [20u8; 6].into();
        let pkt_from: MacAddress = [30u8; 6].into();
        let our_mac: MacAddress = [99u8; 6].into();

        // Insert initial heartbeat to create state
        let hb = Heartbeat::new(Duration::from_millis(1), 7u32, hb_source);
        let initial = Message::new(pkt_from, [255u8; 6].into(), PacketType::Control(Control::Heartbeat(hb.clone())));
        let _ = routing.handle_heartbeat(&initial, our_mac).expect("handled");

        // Create a HeartbeatReply from some sender (not equal to next_upstream)
        let reply_sender: MacAddress = [42u8; 6].into();
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [55u8; 6].into();
        let reply_msg = Message::new(reply_from, [255u8; 6].into(), PacketType::Control(Control::HeartbeatReply(hbr.clone())));

        let res = routing.handle_heartbeat_reply(&reply_msg, our_mac).expect("handled reply");
        // Should return an Ok(Some(_)) with a wire reply
        assert!(res.is_some());
        let out = res.unwrap();
        assert!(!out.is_empty());
    }
}

#[cfg(test)]
mod more_tests {
    use super::Routing;
    use crate::args::{NodeParameters, NodeType};
    use crate::Args;
    use mac_address::MacAddress;

    #[test]
    fn get_route_to_none_when_empty() {
        let args = Args {
            bind: String::default(),
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
        let routing = Routing::new(&args, &boot).expect("routing built");

        let unknown: MacAddress = [1u8; 6].into();
        // No routes yet
        assert!(routing.get_route_to(Some(unknown)).is_none());
        // No cached upstream
        assert!(routing.get_route_to(None).is_none());
    }
}

#[derive(Debug)]
#[allow(clippy::type_complexity)]
pub struct Routing {
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
                IndexMap<Duration, MacAddress>,
                HashMap<MacAddress, Vec<Target>>,
            ),
        >,
    >,
    cached_upstream: ArcSwapOption<MacAddress>,
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
            cached_upstream: ArcSwapOption::from(None),
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

        if let Some((_, _, _hops, _, _)) = entry.get(&message.id()) {
            return Ok(None);
            // So this makes us prioritize hops instead of latency
            // TODO: Is that preferable
            // if _hops < &message.hops() {
            //     return Ok(None);
            // }
        }

        let duration = Instant::now().duration_since(self.boot);
        entry.insert(
            message.id(),
            (
                duration,
                pkt.from()?,
                message.hops(),
                IndexMap::new(),
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
                duration,
                pkt.from()?,
                1,
                IndexMap::new(),
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

        let Some((duration, next_upstream, _, _, downstream)) =
            source_entries.get_mut(&message.id())
        else {
            bail!("no recollection of the next hop for this route");
        };

        if *next_upstream == pkt.from()? {
            bail!("loop detected");
        }

        let seen_at = Instant::now().duration_since(self.boot);
        let latency = seen_at - *duration;
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
            return self.cached_upstream.load().as_ref().map(|mac| Route {
                hops: 1,
                mac: **mac,
                latency: None,
            });
        };
        let mut upstream_routes: Vec<_> = self
            .routes
            .iter()
            .flat_map(|(rsu_mac, seqs)| {
                seqs.iter()
                    .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
            })
            .filter(|(_, rsu_mac, _, _)| rsu_mac == &&mac)
            .collect();
        upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));

        let cached = self.cached_upstream.load();
        if let Some(cached_upstream) = cached.as_ref() {
            if let Some((_, _, upstream_route, hops)) = upstream_routes
                .iter()
                .find(|(_, rsu_mac, _, _)| **rsu_mac == **cached_upstream)
            {
                return Some(Route {
                    hops: **hops,
                    mac: **upstream_route,
                    latency: None,
                });
            }
        }

        std::mem::drop(cached);
        if let Some((_, _, upstream_route, hops)) = upstream_routes.first() {
            let upstream_route = **upstream_route;
            self.cached_upstream.store(Some(upstream_route.into()));
            return Some(Route {
                hops: **hops,
                mac: upstream_route,
                latency: None,
            });
        }

        let route_options: IndexMap<_, _> = self
            .routes
            .iter()
            .flat_map(|(rsus, im)| {
                im.iter().map(move |(seq, (dur, mac, hops, _, rout))| {
                    (seq, (dur, mac, hops, rout, rsus))
                })
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
