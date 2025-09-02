use crate::control::node::ReplyType;
use crate::{
    control::route::Route,
    messages::{
        control::{heartbeat::HeartbeatReply, Control},
        message::Message,
        packet_type::PacketType,
    },
    Args,
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
            control::heartbeat::Heartbeat, control::heartbeat::HeartbeatReply, control::Control,
            message::Message, packet_type::PacketType,
        },
        Args,
    };
    // ReplyType is not used in these test helpers; remove unused import.
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
        let msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

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
        let initial = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .handle_heartbeat(&initial, our_mac)
            .expect("handled");

        // Create a HeartbeatReply from some sender (not equal to next_upstream)
        let reply_sender: MacAddress = [42u8; 6].into();
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [55u8; 6].into();
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        let res = routing
            .handle_heartbeat_reply(&reply_msg, our_mac)
            .expect("handled reply");
        // Should return an Ok(Some(_)) with a wire reply
        assert!(res.is_some());
        let out = res.unwrap();
        assert!(!out.is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::Routing;
    use crate::{
        args::{NodeParameters, NodeType},
        Args,
    };
    use mac_address::MacAddress;

    #[test]
    fn select_and_cache_upstream_sets_cache() {
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
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Create a heartbeat to populate routes
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );
        // Insert heartbeat via routing handle
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now select and cache the upstream for hb_source
        let selected = routing.select_and_cache_upstream(hb_source);
        assert!(selected.is_some());

        // get_route_to(None) should now return the cached upstream route
        let cached = routing.get_route_to(None);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().mac, selected.unwrap().mac);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::Routing;
    use crate::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use crate::messages::{control::Control, message::Message, packet_type::PacketType};
    use crate::{
        args::{NodeParameters, NodeType},
        Args,
    };
    use mac_address::MacAddress;

    // Regression test for the case where a HeartbeatReply arrives from the
    // recorded next hop (pkt.from() == next_upstream). Previously the code
    // treated that as a loop and bailed; that's incorrect. We should only
    // bail if the recorded next_upstream equals the HeartbeatReply's
    // reported sender (message.sender()). This test asserts we do not bail
    // when pkt.from() == next_upstream but message.sender() != next_upstream.
    #[test]
    fn heartbeat_reply_from_next_hop_does_not_bail() {
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
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Heartbeat originates from A, observed via B (pkt.from)
        let hb_source: MacAddress = [1u8; 6].into(); // A
        let pkt_from: MacAddress = [2u8; 6].into(); // B (next hop)
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        // Insert heartbeat to establish next_upstream for hb_source = pkt_from
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now construct a HeartbeatReply where the HeartbeatReply::sender() is A
        // but the packet is from B (pkt.from == next_upstream). This should be
        // accepted and not cause bail.
        let reply_sender: MacAddress = hb_source; // A
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = pkt_from; // B
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        // When the reply arrives from the next hop (pkt.from == next_upstream)
        // we should NOT forward it back (to avoid an immediate bounce). The
        // function returns Ok(None) in this case.
        let res = routing
            .handle_heartbeat_reply(&reply_msg, our_mac)
            .expect("handled reply");
        assert!(res.is_none());
    }

    #[test]
    fn heartbeat_reply_from_sender_triggers_bail() {
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
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Heartbeat originates from A, observed via B (pkt.from)
        let hb_source: MacAddress = [10u8; 6].into(); // A
        let pkt_from: MacAddress = [20u8; 6].into(); // B (next hop)
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 2u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        // Insert heartbeat to establish next_upstream for hb_source = pkt_from
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now construct a HeartbeatReply where the HeartbeatReply::sender() is
        // equal to our recorded next_upstream (i.e., message.sender == next_upstream)
        let reply_sender: MacAddress = pkt_from; // B == next_upstream
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [30u8; 6].into(); // some other node forwarded it
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        // This should bail with an error indicating a loop was detected.
        let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(format!("{}", err).contains("loop detected"));
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

    /// Return the cached upstream MAC if present.
    pub fn get_cached_upstream(&self) -> Option<MacAddress> {
        self.cached_upstream.load().as_ref().map(|m| **m)
    }

    /// Clear the cached upstream (useful when topology changes) and increment metric.
    pub fn clear_cached_upstream(&self) {
        tracing::trace!("clearing cached_upstream");
        self.cached_upstream.store(None);
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_clear();
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
                    // Invalidate cached upstream when route changes
                    self.clear_cached_upstream();
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
                        // Invalidate cached upstream when route changes
                        self.clear_cached_upstream();
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

        // Note: avoid forwarding the HeartbeatReply back to the node it came
        // from. If `pkt.from()` equals our recorded `next_upstream`, sending a
        // reply to `next_upstream` would immediately bounce the packet back and
        // can create a forwarding loop. We'll still record downstream
        // observations below, but skip forwarding in that case.

        // Decide action and emit a trace-level log so we can inspect decisions
        // in live runs. Action values:
        //  - "bail" : next_upstream == message.sender() (genuine loop)
        //  - "skip_forward" : pkt.from == next_upstream (would bounce)
        //  - "forward" : safe to forward toward next_upstream
        let pkt_from = pkt.from()?;
        let sender = message.sender();
        let action = if *next_upstream == sender {
            "bail"
        } else if pkt_from == *next_upstream {
            "skip_forward"
        } else {
            "forward"
        };

        tracing::trace!(
            pkt_from = %pkt_from,
            message_sender = %sender,
            next_upstream = %next_upstream,
            action = %action,
            "heartbeat_reply decision"
        );

        if action == "bail" {
            #[cfg(feature = "stats")]
            crate::metrics::inc_loop_detected();
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

        // If the reply arrived from the node we'd forward to, don't forward
        // it back: that would produce an immediate bounce. Drop forwarding
        // (but keep the recorded downstream information above).
        if pkt.from()? == *next_upstream {
            return Ok(None);
        }

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
                    // Invalidate cached upstream when route changes
                    self.clear_cached_upstream();
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
                        // Invalidate cached upstream when route changes
                        self.clear_cached_upstream();
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

        // NOTE: This function is intentionally pure for the `Some(mac)` case:
        // it computes a best-route without mutating the cached upstream. Callers
        // who want to select-and-cache must use `select_and_cache_upstream`.
        if let Some((_, _, upstream_route, hops)) = upstream_routes.first() {
            let upstream_route = **upstream_route;
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

        // Compute deterministic integer-based metrics for latency in microseconds.
        // For each candidate MAC, compute:
        //  - min_latency_us: minimum observed latency in microseconds
        //  - avg_latency_us: average observed latency in microseconds
        // We then select the candidate with the smallest (min_latency_us + avg_latency_us) value
        // (sum) as an integer tie-breaker. This avoids floating point rounding and is deterministic.
        let route_options: IndexMap<_, _> = route_options
            .iter()
            .filter(|(h, _)| h == &min_hops)
            .flat_map(|(hops, (_count, _min_seq, next, latency))| {
                let hop_val = *hops as u32;
                latency.iter().zip(next).fold(
                    HashMap::default(),
                    |mut hm: HashMap<MacAddress, (u128, u128, u32, u32)>, (val, mac)| {
                        // val is Option<micros> (reference)
                        if let Some(v) = *val {
                            let micros = v as u128;
                            let entry = hm.entry(*mac).or_insert((u128::MAX, 0u128, 0u32, hop_val));
                            if entry.0 > micros {
                                entry.0 = micros; // min
                            }
                            entry.1 += micros; // sum
                            entry.2 += 1; // count
                                          // hops is already stored in entry.3
                        }
                        hm
                    },
                )
            })
            .map(|(mac, (min_us, sum_us, n, hops_val))| {
                let avg_us = if n > 0 {
                    sum_us / (n as u128)
                } else {
                    u128::MAX
                };
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
            .map(|(_, (mac, hops, latency_us))| Route {
                hops: *hops,
                mac: *mac,
                latency: Some(Duration::from_micros((*latency_us) as u64)),
            })
    }

    /// Compute the best route to `mac` and store it in the cached upstream.
    /// This is the write API callers should use when they want selection to
    /// also update the cached upstream. This separates the pure selection
    /// logic (above) from the side-effect of caching.
    pub fn select_and_cache_upstream(&self, mac: MacAddress) -> Option<Route> {
        let route = self.get_route_to(Some(mac))?;
        self.cached_upstream.store(Some(route.mac.into()));
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_select();
        Some(route)
    }
}
