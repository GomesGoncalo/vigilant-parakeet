use crate::{
    control::{node::ReplyType, route::Route},
    messages::{
        control::{heartbeat::Heartbeat, Control},
        message::Message,
        packet_type::PacketType,
    },
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
        let message = Heartbeat::new(
            Instant::now().duration_since(self.boot),
            self.hb_seq,
            address,
        );

        if self.sent.first().is_some_and(|(x, _)| x > &message.id()) {
            self.sent.clear();
        }

        if self.sent.len() == self.sent.capacity() && self.sent.capacity() > 0 {
            self.sent.swap_remove_index(0);
        }

        let _ = self
            .sent
            .insert(message.id(), (message.duration(), HashMap::default()));

        self.hb_seq += 1;

        let msg = Message::new(
            address,
            [255; 6].into(),
            PacketType::Control(Control::Heartbeat(message)),
        );

        msg
    }

    pub fn handle_heartbeat_reply(
        &mut self,
        msg: &Message,
        address: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::HeartbeatReply(hbr)) = msg.get_packet_type() else {
            bail!("only heartbeat reply messages accepted");
        };

        let old_route = self.get_route_to(Some(hbr.sender()));
        let Some((_, map)) = self.sent.get_mut(&hbr.id()) else {
            tracing::warn!("outdated heartbeat");
            return Ok(None);
        };

        let latency = Instant::now().duration_since(self.boot) - hbr.duration();
        match map.entry(hbr.sender()) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();

                value.push(Target {
                    hops: hbr.hops(),
                    mac: msg.from()?,
                    latency,
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![Target {
                    hops: hbr.hops(),
                    mac: msg.from()?,
                    latency,
                }]);
            }
        };

        let new_route = self.get_route_to(Some(hbr.sender()));
        let Some(ref new_route) = new_route else {
            return Ok(None);
        };

        match (old_route, new_route) {
            (None, new_route) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %address,
                    to = %hbr.sender(),
                    through = %new_route,
                    "route created from heartbeat reply",
                );
            }
            (Some(old_route), new_route) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %address,
                        to = %hbr.sender(),
                        through = %new_route,
                        was_through = %old_route,
                        "route changed from heartbeat reply",
                    );
                }
            }
        }

        Ok(None)
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

        // Compute deterministic integer-based metrics for latency in microseconds.
        // For each candidate MAC, compute min and average latency in microseconds and
        // use (min + avg) as a deterministic integer score for selection. This avoids
        // floating point rounding differences and is easier to test.
        let route_options: IndexMap<_, _> = route_options
            .iter()
            .filter(|(h, _)| h == &min_hops)
            .flat_map(|(hops, (_count, _min_seq, next, latency))| {
                let hops_val = *hops as u32;
                latency.iter().zip(next).fold(
                    HashMap::default(),
                    |mut hm: HashMap<MacAddress, (u128, u128, u32, u32)>, (val, mac)| {
                        // here val: &u128
                        let micros = *val as u128;
                        let entry = hm.entry(*mac).or_insert((u128::MAX, 0u128, 0u32, hops_val));
                        if entry.0 > micros {
                            entry.0 = micros;
                        }
                        entry.1 += micros;
                        entry.2 += 1;
                        hm
                    },
                )
            })
            .map(|(mac, (min_us, sum_us, n, hops_val))| {
                let avg_us = if n > 0 { sum_us / (n as u128) } else { u128::MAX };
                let score = if min_us == u128::MAX || avg_us == u128::MAX {
                    u128::MAX
                } else {
                    min_us + avg_us
                };
                (score as usize, (mac, hops_val, avg_us))
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
    use crate::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use crate::{
        args::{NodeParameters, NodeType},
        control::rsu::Routing,
        messages::{control::Control, message::Message, packet_type::PacketType},
        Args,
    };
    use mac_address::MacAddress;
    use std::time::Duration;

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

        assert_eq!(message.from().expect(""), [1; 6].into());
        assert_eq!(message.to().expect(""), [255; 6].into());

        let PacketType::Control(Control::Heartbeat(hb)) = message.get_packet_type() else {
            panic!("did not generate a heartbeat");
        };

        assert_eq!(hb.source(), [1; 6].into());
        assert_eq!(hb.hops(), 0);
        assert_eq!(hb.id(), 0);

        let message: Vec<Vec<u8>> = (&message).into();
        let message: Vec<u8> = message.iter().flat_map(|x| x.iter()).cloned().collect();
        let message: Message = (&message[..]).try_into().expect("same message");
        assert_eq!(message.from().expect(""), [1; 6].into());
        assert_eq!(message.to().expect(""), [255; 6].into());
        let PacketType::Control(Control::Heartbeat(hb)) = message.get_packet_type() else {
            panic!("did not generate a heartbeat");
        };

        assert_eq!(hb.source(), [1; 6].into());
        assert_eq!(hb.hops(), 1);
        assert_eq!(hb.id(), 0);
    }

    #[test]
    fn rsu_handle_heartbeat_reply_inserts_route() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let Ok(mut routing) = Routing::new(&args) else {
            panic!("did not build a routing object");
        };

        // use send_heartbeat to create initial state for the given rsu source
        let src: MacAddress = [101u8; 6].into();
        let _ = routing.send_heartbeat(src);

        // the first heartbeat inserted will have id 0, construct a matching heartbeat
        let hb = Heartbeat::new(Duration::from_millis(0), 0u32, src);
        let reply_sender: MacAddress = [200u8; 6].into();
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [201u8; 6].into();
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        let res = routing
            .handle_heartbeat_reply(&reply_msg, [103u8; 6].into())
            .expect("handled reply");
        // implementation returns Ok(None) for this code path, ensure no reply and that route exists
        assert!(res.is_none());

        let route = routing.get_route_to(Some(reply_sender));
        assert!(route.is_some());
    }
}

#[cfg(test)]
mod more_tests {
    use super::Routing;
    use crate::args::{NodeParameters, NodeType};
    use crate::Args;
    use mac_address::MacAddress;

    #[test]
    fn iter_next_hops_empty_and_get_route_none_when_empty() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let routing = Routing::new(&args).expect("routing built");

        // iter_next_hops should be empty
        assert_eq!(routing.iter_next_hops().count(), 0);

        let unknown: MacAddress = [9u8; 6].into();
        assert!(routing.get_route_to(Some(unknown)).is_none());
    }
}
