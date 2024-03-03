use crate::{
    control::route::Route,
    messages::{ControlType, HeartBeat, Message, PacketType},
    Args,
};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use itertools::Itertools;
use mac_address::MacAddress;
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    time::{Duration, Instant},
};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Duration,
}

#[derive(Debug)]
pub struct Routing {
    hb_seq: u32,
    boot: Instant,
    sent: IndexMap<u32, (Duration, HashMap<MacAddress, Vec<Target>>)>,
}

impl Routing {
    pub fn new(args: &Args) -> Result<Self> {
        if args.node_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            hb_seq: 0,
            boot: Instant::now(),
            sent: IndexMap::with_capacity(usize::try_from(args.node_params.hello_history)?),
        })
    }

    pub fn send_heartbeat(&mut self, address: MacAddress) -> Message {
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

    pub fn handle_heartbeat_reply(&mut self, msg: &Message, address: MacAddress) -> Result<()> {
        let Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) = msg.next_layer() else {
            bail!("only heartbeat reply messages accepted");
        };

        let old_route = self.get_route_to(Some(hbr.sender));
        let Some((_, map)) = self.sent.get_mut(&hbr.id) else {
            tracing::warn!("outdated heartbeat");
            return Ok(());
        };

        let latency = Instant::now().duration_since(self.boot) - hbr.now;
        match map.entry(hbr.sender) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(Target {
                    hops: hbr.hops,
                    mac: msg.from(),
                    latency,
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![Target {
                    hops: hbr.hops,
                    mac: msg.from(),
                    latency,
                }]);
            }
        };

        match (old_route, self.get_route_to(Some(hbr.sender))) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %address,
                    to = %hbr.sender,
                    through = %new_route,
                    "route created from heartbeat reply",
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
                        "route changed from heartbeat reply",
                    );
                }
            }
        }
        Ok(())
    }

    pub fn get_route_to(&self, mac: Option<MacAddress>) -> Option<Route> {
        let mac = mac?;
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

    pub fn iter_next_hops(&self) -> impl Iterator<Item = &MacAddress> {
        self.sent.iter().flat_map(|(_, (_, m))| m.keys()).unique()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        control::{
            args::{NodeParameters, NodeType},
            rsu::Routing,
        },
        messages::{ControlType, PacketType},
        Args,
    };

    #[test]
    fn can_generate_heartbeat() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
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

        assert_eq!(message.from(), [1; 6].into());
        assert_eq!(message.to(), [255; 6].into());

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
            mtu: 1500,
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
                mtu: 1500,
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
