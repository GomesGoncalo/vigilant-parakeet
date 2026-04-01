use super::{node::ReplyType, route::Route};
use crate::args::RsuArgs;
use anyhow::{bail, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use node_lib::messages::{
    control::{heartbeat::Heartbeat, Control},
    message::Message,
    packet_type::PacketType,
};
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    fmt::Debug,
};
use tokio::time::{Duration, Instant};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Duration,
}

/// Sliding-receive-window replay detector (IPsec AH style), tracked per sender.
///
/// Maintains the highest accepted sequence number and a 64-bit bitmask of
/// recently accepted values. A sequence number is accepted if and only if it
/// is strictly greater than `last_seq - WIDTH` and has not been accepted before.
#[derive(Debug, Default)]
struct ReplayWindow {
    last_seq: u32,
    /// Bit `i` is set when sequence `last_seq - i` has been accepted.
    window: u64,
    initialized: bool,
}

impl ReplayWindow {
    const WIDTH: u32 = 64;

    /// Returns `true` and records `seq` if it is fresh; returns `false` if it
    /// is a replay or falls outside the window (too old).
    fn check_and_update(&mut self, seq: u32) -> bool {
        if !self.initialized {
            self.last_seq = seq;
            self.window = 1;
            self.initialized = true;
            return true;
        }

        if seq > self.last_seq {
            let advance = seq - self.last_seq;
            self.window = if advance >= Self::WIDTH {
                1
            } else {
                (self.window << advance) | 1
            };
            self.last_seq = seq;
            true
        } else {
            let diff = self.last_seq - seq;
            if diff >= Self::WIDTH {
                return false; // outside window — too old
            }
            let bit = 1u64 << diff;
            if self.window & bit != 0 {
                return false; // already seen — replay
            }
            self.window |= bit;
            true
        }
    }
}

#[derive(Debug)]
pub struct Routing {
    hb_seq: u32,
    boot: Instant,
    sent: VecDeque<(u32, Duration, HashMap<MacAddress, Vec<Target>>)>,
    max_history: usize,
    replay_windows: HashMap<MacAddress, ReplayWindow>,
}

impl Routing {
    pub fn new(args: &RsuArgs) -> Result<Self> {
        if args.rsu_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        let max_history = usize::try_from(args.rsu_params.hello_history)?;
        Ok(Self {
            hb_seq: 0,
            boot: Instant::now(),
            sent: VecDeque::with_capacity(max_history),
            max_history,
            replay_windows: HashMap::default(),
        })
    }

    pub fn send_heartbeat(&mut self, address: MacAddress) -> Message<'_> {
        let message = Heartbeat::new(
            Instant::now().duration_since(self.boot),
            self.hb_seq,
            address,
        );

        // Handle sequence wraparound: if the first entry has a higher seq than current, clear all
        if self.sent.front().is_some_and(|(x, _, _)| x > &message.id()) {
            self.sent.clear();
            self.replay_windows.clear();
        }

        // Maintain fixed-size history with O(1) pop_front
        if self.sent.len() >= self.max_history {
            self.sent.pop_front();
        }

        self.sent
            .push_back((message.id(), message.duration(), HashMap::default()));

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

        let sender = hbr.sender();
        let reply_id = hbr.id();

        // Reject IDs not present in our sent history before touching the replay
        // window. Without this guard an attacker could forge a reply with
        // id=u32::MAX, advance last_seq to u32::MAX, and cause all subsequent
        // legitimate replies from that sender to fall outside the window.
        let sent_idx = self.sent.iter().position(|(id, _, _)| *id == reply_id);
        let Some(sent_idx) = sent_idx else {
            tracing::debug!(
                reply_id,
                %sender,
                "Ignoring outdated heartbeat reply"
            );
            return Ok(None);
        };

        if !self
            .replay_windows
            .entry(sender)
            .or_default()
            .check_and_update(reply_id)
        {
            tracing::debug!(
                reply_id,
                %sender,
                "Dropping replayed heartbeat reply"
            );
            return Ok(None);
        }

        let old_route = self.get_route_to(Some(sender));

        // Extract all hbr fields before the mutable borrow of self.sent.
        let hops = hbr.hops();
        let duration = hbr.duration();
        let from_mac = msg.from()?;

        {
            let (_, _, map) = &mut self.sent[sent_idx];
            let latency = Instant::now().duration_since(self.boot) - duration;
            match map.entry(sender) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().push(Target {
                        hops,
                        mac: from_mac,
                        latency,
                    });
                }
                Entry::Vacant(entry) => {
                    entry.insert(vec![Target {
                        hops,
                        mac: from_mac,
                        latency,
                    }]);
                }
            }
        } // mutable borrow of self.sent released here

        let new_route = self.get_route_to(Some(sender));
        let Some(ref new_route) = new_route else {
            return Ok(None);
        };

        match (old_route, new_route) {
            (None, new_route) => {
                tracing::event!(
                    Level::INFO,
                    from = %address,
                    to = %sender,
                    through = %new_route,
                    hops = new_route.hops,
                    "Route discovered",
                );
            }
            (Some(old_route), new_route) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::INFO,
                        from = %address,
                        to = %sender,
                        through = %new_route,
                        was_through = %old_route,
                        old_hops = old_route.hops,
                        new_hops = new_route.hops,
                        "Route changed",
                    );
                }
            }
        }

        Ok(None)
    }

    pub fn get_route_to(&self, mac: Option<MacAddress>) -> Option<Route> {
        let target = mac?;

        // Collect all observed candidates for the target: (hops, next_hop_mac, latency_us)
        let mut candidates: Vec<(u32, MacAddress, u128)> = Vec::new();
        for (_seq, _dur, m) in self.sent.iter().rev() {
            if let Some(routes) = m.get(&target) {
                for r in routes {
                    candidates.push((r.hops, r.mac, r.latency.as_micros()));
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }

        // Determine the true minimum hop count across candidates
        let min_hops = candidates
            .iter()
            .map(|(h, _, _)| *h)
            .min()
            .expect("candidates is non-empty, min must exist");

        // Aggregate per-next-hop metrics into latency_candidates and pick the best
        // using the shared helper for parity with OBU.
        let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> = HashMap::new();
        for (_hops, next_hop_mac, latency_us) in
            candidates.into_iter().filter(|(h, _, _)| *h == min_hops)
        {
            let entry = latency_candidates.entry(next_hop_mac).or_insert((
                u128::MAX,
                0u128,
                0u32,
                min_hops,
            ));
            if latency_us < entry.0 {
                entry.0 = latency_us;
            }
            entry.1 += latency_us;
            entry.2 += 1;
            entry.3 = min_hops;
        }

        let (mac, avg_us) =
            crate::control::routing_utils::pick_best_from_latency_candidates(latency_candidates)?;
        Some(Route {
            hops: min_hops,
            mac,
            latency: if avg_us == u128::MAX {
                None
            } else {
                Some(Duration::from_micros(avg_us as u64))
            },
        })
    }

    pub fn iter_next_hops(&self) -> impl Iterator<Item = &MacAddress> {
        self.sent.iter().flat_map(|(_, _, m)| m.keys()).unique()
    }
}

#[cfg(test)]
mod tests {
    use crate::args::{RsuArgs, RsuParameters};
    use crate::control::Routing;
    use mac_address::MacAddress;
    use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
    use std::time::Duration;

    #[test]
    fn can_generate_heartbeat() {
        let args = RsuArgs {
            bind: String::default(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 1,
                hello_periodicity: 1000, // RSU requires hello_periodicity
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
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

        // Use flat serialization - simpler and faster
        let message: Vec<u8> = (&message).into();
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
        let args = RsuArgs {
            bind: String::default(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000, // RSU requires hello_periodicity
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
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
    use crate::args::{RsuArgs, RsuParameters};
    use mac_address::MacAddress;

    #[test]
    fn iter_next_hops_empty_and_get_route_none_when_empty() {
        let args = RsuArgs {
            bind: String::default(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 2,
                hello_periodicity: 1000, // RSU requires hello_periodicity
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        };

        let routing = Routing::new(&args).expect("routing built");

        // iter_next_hops should be empty
        assert_eq!(routing.iter_next_hops().count(), 0);

        let unknown: MacAddress = [9u8; 6].into();
        assert!(routing.get_route_to(Some(unknown)).is_none());
    }
}

#[cfg(test)]
mod replay_window_tests {
    use super::ReplayWindow;

    #[test]
    fn fresh_sequence_accepted() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(1));
    }

    #[test]
    fn same_sequence_rejected_as_replay() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(5));
        assert!(!w.check_and_update(5));
    }

    #[test]
    fn monotonically_increasing_sequences_accepted() {
        let mut w = ReplayWindow::default();
        for seq in 0..10 {
            assert!(w.check_and_update(seq), "seq {seq} should be accepted");
        }
    }

    #[test]
    fn out_of_order_within_window_accepted_once() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(10));
        assert!(w.check_and_update(8)); // within window, not seen yet
        assert!(!w.check_and_update(8)); // same — replay
        assert!(w.check_and_update(9)); // another within-window fresh value
    }

    #[test]
    fn sequence_outside_window_rejected() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(100));
        // 100 - 64 = 36 is just at the boundary (diff == WIDTH), rejected
        assert!(!w.check_and_update(36));
        // 37 is still within window (diff == 63)
        assert!(w.check_and_update(37));
    }

    #[test]
    fn large_advance_clears_window() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(1));
        assert!(w.check_and_update(2));
        // Jump far ahead — old entries fall out of window
        assert!(w.check_and_update(200));
        // seq=1 is now outside window
        assert!(!w.check_and_update(1));
    }

    #[test]
    fn sequence_zero_is_valid_first_entry() {
        let mut w = ReplayWindow::default();
        assert!(w.check_and_update(0));
        assert!(!w.check_and_update(0));
        assert!(w.check_and_update(1));
    }
}

#[cfg(test)]
mod replay_integration_tests {
    use super::Routing;
    use crate::args::{RsuArgs, RsuParameters};
    use mac_address::MacAddress;
    use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
    use std::time::Duration;

    fn make_args(hello_history: u32) -> RsuArgs {
        RsuArgs {
            bind: String::default(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history,
                hello_periodicity: 1000,
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        }
    }

    /// Serialises a HeartbeatReply to wire bytes so tests can round-trip through
    /// `Message::try_from` without lifetime complications.
    fn make_reply_wire(hb_id: u32, sender: MacAddress, from: MacAddress) -> Vec<u8> {
        let hb = Heartbeat::new(Duration::from_millis(0), hb_id, [1u8; 6].into());
        let hbr = HeartbeatReply::from_sender(&hb, sender);
        let msg = Message::new(
            from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr)),
        );
        (&msg).into()
    }

    #[test]
    fn replayed_reply_does_not_insert_duplicate_route() {
        let mut routing = Routing::new(&make_args(2)).expect("routing built");
        let rsu_mac: MacAddress = [1u8; 6].into();
        let _ = routing.send_heartbeat(rsu_mac); // heartbeat id=0

        let sender: MacAddress = [200u8; 6].into();
        let from: MacAddress = [201u8; 6].into();
        let wire = make_reply_wire(0, sender, from);

        // First delivery — accepted, route inserted
        let msg = Message::try_from(&wire[..]).expect("parse");
        let r1 = routing.handle_heartbeat_reply(&msg, rsu_mac).unwrap();
        assert!(r1.is_none());
        assert!(routing.get_route_to(Some(sender)).is_some());

        // Replayed delivery — silently dropped
        let msg = Message::try_from(&wire[..]).expect("parse");
        let r2 = routing.handle_heartbeat_reply(&msg, rsu_mac).unwrap();
        assert!(r2.is_none());

        // Route still exists (was not removed) but no duplicate targets were added
        assert!(routing.get_route_to(Some(sender)).is_some());
    }

    #[test]
    fn independent_senders_tracked_separately() {
        let mut routing = Routing::new(&make_args(2)).expect("routing built");
        let rsu_mac: MacAddress = [1u8; 6].into();
        let _ = routing.send_heartbeat(rsu_mac); // heartbeat id=0

        let sender_a: MacAddress = [10u8; 6].into();
        let sender_b: MacAddress = [20u8; 6].into();
        let from: MacAddress = [99u8; 6].into();

        let wire_a = make_reply_wire(0, sender_a, from);
        let wire_b = make_reply_wire(0, sender_b, from);

        // Both senders reply to heartbeat 0 — both accepted
        let msg = Message::try_from(&wire_a[..]).expect("parse");
        assert!(routing.handle_heartbeat_reply(&msg, rsu_mac).is_ok());
        let msg = Message::try_from(&wire_b[..]).expect("parse");
        assert!(routing.handle_heartbeat_reply(&msg, rsu_mac).is_ok());

        // Replays from both senders are rejected
        let msg = Message::try_from(&wire_a[..]).expect("parse");
        assert!(routing
            .handle_heartbeat_reply(&msg, rsu_mac)
            .unwrap()
            .is_none());
        let msg = Message::try_from(&wire_b[..]).expect("parse");
        assert!(routing
            .handle_heartbeat_reply(&msg, rsu_mac)
            .unwrap()
            .is_none());

        // Routes for both senders still exist
        assert!(routing.get_route_to(Some(sender_a)).is_some());
        assert!(routing.get_route_to(Some(sender_b)).is_some());
    }

    #[test]
    fn forged_large_id_does_not_poison_replay_window() {
        // An attacker injects a reply with id=u32::MAX. This ID is not in
        // `sent`, so it must be rejected without advancing last_seq. If it
        // were accepted, all subsequent legitimate replies (with small IDs)
        // would fall outside the window and be silently dropped.
        let mut routing = Routing::new(&make_args(3)).expect("routing built");
        let rsu_mac: MacAddress = [1u8; 6].into();
        let sender: MacAddress = [50u8; 6].into();
        let from: MacAddress = [51u8; 6].into();

        let _ = routing.send_heartbeat(rsu_mac); // id=0

        // Inject a forged reply with id=u32::MAX (not in sent history)
        let forged_wire = make_reply_wire(u32::MAX, sender, from);
        let msg = Message::try_from(&forged_wire[..]).expect("parse");
        let r = routing.handle_heartbeat_reply(&msg, rsu_mac).unwrap();
        assert!(r.is_none());

        // Legitimate reply for id=0 must still be accepted
        let wire0 = make_reply_wire(0, sender, from);
        let msg = Message::try_from(&wire0[..]).expect("parse");
        routing
            .handle_heartbeat_reply(&msg, rsu_mac)
            .expect("should not error");
        assert!(
            routing.get_route_to(Some(sender)).is_some(),
            "route must be present after legitimate reply"
        );
    }

    #[test]
    fn advancing_heartbeat_id_accepted_per_sender() {
        let mut routing = Routing::new(&make_args(3)).expect("routing built");
        let rsu_mac: MacAddress = [1u8; 6].into();
        let sender: MacAddress = [50u8; 6].into();
        let from: MacAddress = [51u8; 6].into();

        // Send two heartbeats (id=0 and id=1)
        let _ = routing.send_heartbeat(rsu_mac);
        let _ = routing.send_heartbeat(rsu_mac);

        // Reply to heartbeat 0 — accepted
        let wire0 = make_reply_wire(0, sender, from);
        let msg = Message::try_from(&wire0[..]).expect("parse");
        assert!(routing.handle_heartbeat_reply(&msg, rsu_mac).is_ok());

        // Reply to heartbeat 1 — also accepted (higher id)
        let wire1 = make_reply_wire(1, sender, from);
        let msg = Message::try_from(&wire1[..]).expect("parse");
        assert!(routing.handle_heartbeat_reply(&msg, rsu_mac).is_ok());

        // Replay of heartbeat 0 reply — rejected (within window, already seen)
        let msg = Message::try_from(&wire0[..]).expect("parse");
        let replay = routing.handle_heartbeat_reply(&msg, rsu_mac).unwrap();
        assert!(replay.is_none());
    }
}
