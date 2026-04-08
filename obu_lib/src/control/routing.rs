//! OBU Routing Implementation
//!
//! This module implements routing logic for OBU (On-Board Unit) nodes in a vehicular network.
//! It handles heartbeat-based topology discovery, route selection with latency awareness,
//! and failover management.
//!
//! ## Module Structure (1,032 lines)
//!
//! - **Type Definitions** (~50 lines): Core types and aliases
//! - **Routing struct** (~980 lines): Implementation organized in 4 sections:
//!   1. Construction and cache operations
//!   2. Failover and candidate management  
//!   3. Heartbeat message processing (382 lines)
//!   4. Route selection with hysteresis (424 lines)
//!
//! ## Test Organization
//!
//! Tests are extracted to separate files in `routing/`:
//! - `failover_tests.rs` (233 lines): Candidate rebuild and failover logic
//! - `heartbeat_tests.rs` (102 lines): Heartbeat message processing
//! - `cache_tests.rs` (112 lines): Upstream caching functionality
//! - `regression_tests.rs` (123 lines): Loop detection edge cases
//! - `selection_tests.rs` (270 lines): Route selection with hysteresis
//!
//! ## Key Features
//!
//! - Latency-based route selection with hop-count fallback
//! - Hysteresis to prevent route flapping (10% threshold)
//! - Multi-candidate caching for fast failover
//! - Deterministic tie-breaking for reproducible routing
//!
//! ## Related Modules
//!
//! - `routing_cache`: Lock-free cache management (extracted, 229 lines)
//! - `routing_utils`: Shared routing utilities (scoring, selection)
//! - `route`: Route data structure

use super::{node::ReplyType, route::Route, routing_cache::RoutingCache};
use crate::args::ObuArgs;
use anyhow::{bail, Result};
use indexmap::IndexMap;
use mac_address::MacAddress;
use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use tokio::time::{Duration, Instant};

// ============================================================================
// Type Definitions
// ============================================================================

/// Action to take when forwarding a HeartbeatReply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardAction {
    /// next_upstream == message.sender() with no viable alternative — drop.
    Bail,
    /// pkt.from == next_upstream — would bounce, skip forwarding.
    SkipForward,
    /// Safe to forward toward next_upstream.
    Forward,
    /// next_upstream == sender (would loop), but cached/alternative upstream
    /// provides a viable forwarding path.
    ForwardCached,
}

// Type aliases for complex routing table structures
/// Per-hop routing information with latency measurements
type PerHopInfo = (
    Duration,
    MacAddress,
    u32,
    IndexMap<Duration, MacAddress>,
    HashMap<MacAddress, Vec<Target>>,
);

/// Routing table indexed by sequence number
type SequenceMap = IndexMap<u32, PerHopInfo>;

/// Complete routing table for all known nodes
type RoutingTable = HashMap<MacAddress, SequenceMap>;

#[derive(Debug)]
pub(crate) struct Target {
    pub hops: u32,
    pub mac: MacAddress,
    pub latency: Option<Duration>,
}

#[cfg(test)]
#[path = "routing/failover_tests.rs"]
mod failover_tests;

#[cfg(test)]
#[path = "routing/heartbeat_tests.rs"]
mod heartbeat_tests;

#[cfg(test)]
#[path = "routing/cache_tests.rs"]
mod cache_tests;

#[cfg(test)]
#[path = "routing/regression_tests.rs"]
mod regression_tests;

#[cfg(test)]
#[path = "routing/selection_tests.rs"]
mod selection_tests;

/// Per-neighbor RSSI table (MAC → received signal strength in dBm).
///
/// Populated by the simulator fading task (distance-based) or a real radio
/// driver (hardware-reported RSSI). When present, RSU selection uses RSSI as
/// the primary quality metric instead of the heartbeat reception-ratio.
pub type RssiTable = std::sync::Arc<std::sync::RwLock<HashMap<MacAddress, f32>>>;

pub struct Routing {
    args: ObuArgs,
    boot: Instant,
    routes: RoutingTable,
    cache: RoutingCache,
    // Track distinct neighbors that forwarded heartbeats for a given source (e.g., RSU)
    source_neighbors: HashMap<MacAddress, HashSet<MacAddress>>,
    /// Live RSSI readings, injected by the simulator or a real radio driver.
    rssi_table: Option<RssiTable>,
}

impl Routing {
    pub fn new(args: &ObuArgs, boot: &Instant) -> Result<Self> {
        if args.obu_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            args: args.clone(),
            boot: *boot,
            routes: HashMap::default(),
            cache: RoutingCache::new(args.obu_params.cached_candidates),
            source_neighbors: HashMap::default(),
            rssi_table: None,
        })
    }

    /// Return the cached upstream MAC if present.
    pub fn get_cached_upstream(&self) -> Option<MacAddress> {
        self.cache.get_cached_upstream()
    }

    /// Clear the cached upstream (useful when topology changes) and increment metric.
    pub fn clear_cached_upstream(&self) {
        self.cache.clear()
    }

    /// Return the ordered cached candidates (primary first) when present.
    pub fn get_cached_candidates(&self) -> Option<Vec<MacAddress>> {
        self.cache.get_cached_candidates()
    }

    /// Attach a live RSSI table.
    ///
    /// Once set, `select_and_cache_upstream` uses RSSI (dBm) as the primary
    /// RSU selection metric: a higher value means stronger signal / closer RSU.
    /// A 3 dB hysteresis prevents unnecessary handoffs between equally-good RSUs.
    pub fn set_rssi_table(&mut self, table: RssiTable) {
        self.rssi_table = Some(table);
    }

    /// Rotate to the next cached candidate (promote the next candidate to primary).
    /// Returns the newly promoted primary if any.
    pub fn failover_cached_upstream(&self) -> Option<MacAddress> {
        self.cache.failover(|src, n_best| {
            // Rebuild candidates based on routing table
            let mut cands = Vec::new();

            // Compute latency-based candidates first
            let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
                HashMap::default();
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                    if let Some(vec) = downstream.get(&src) {
                        for route in vec.iter() {
                            if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                                let entry = latency_candidates.entry(route.mac).or_insert((
                                    u128::MAX,
                                    0u128,
                                    0u32,
                                    route.hops,
                                ));
                                if entry.0 > lat {
                                    entry.0 = lat;
                                }
                                entry.1 += lat;
                                entry.2 += 1;
                                entry.3 = route.hops;
                            }
                        }
                    }
                }
            }
            if !latency_candidates.is_empty() {
                // Use the shared helper to score and sort candidates deterministically.
                let scored_full = crate::control::routing_utils::score_and_sort_latency_candidates(
                    latency_candidates,
                );
                cands = scored_full
                    .into_iter()
                    .map(|(_score, _hops, mac, _avg)| mac)
                    .take(n_best)
                    .collect();
            }
            // Backfill with hop-based ordering if needed
            if cands.len() < n_best {
                let mut upstream_routes: Vec<_> = self
                    .routes
                    .iter()
                    .flat_map(|(rsu_mac, seqs)| {
                        seqs.iter()
                            .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
                    })
                    .filter(|(_, rsu_mac, mac, _)| rsu_mac == &&src && *mac != &src)
                    .collect();
                upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));
                let mut seen: std::collections::HashSet<MacAddress> =
                    cands.iter().copied().collect();
                for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                    if !seen.contains(mac_ref) {
                        seen.insert(*mac_ref);
                        cands.push(*mac_ref);
                        if cands.len() >= n_best {
                            break;
                        }
                    }
                }
            }

            cands
        })
    }

    /// Test helper: directly set cached candidates and primary for tests.
    #[cfg(test)]
    pub fn test_set_cached_candidates(&self, cands: Vec<MacAddress>) {
        self.cache.test_set_cached_candidates(cands);
    }

    fn log_route_change(
        old_route: Option<Route>,
        new_route: Option<Route>,
        from_mac: MacAddress,
        to_mac: MacAddress,
        is_info: bool,
        context: &str,
    ) {
        match (old_route, new_route) {
            (None, Some(route)) => {
                if is_info {
                    tracing::info!(
                        from = %from_mac,
                        to = %to_mac,
                        through = %route,
                        hops = route.hops,
                        "{}", context,
                    );
                } else {
                    tracing::debug!(
                        from = %from_mac,
                        to = %to_mac,
                        through = %route,
                        hops = route.hops,
                        "{}", context,
                    );
                }
            }
            (Some(old), Some(new)) if old.mac != new.mac => {
                if is_info {
                    tracing::info!(
                        from = %from_mac,
                        to = %to_mac,
                        through = %new,
                        was_through = %old,
                        old_hops = old.hops,
                        new_hops = new.hops,
                        "Route changed",
                    );
                } else {
                    tracing::debug!(
                        from = %from_mac,
                        to = %to_mac,
                        through = %new,
                        was_through = %old,
                        old_hops = old.hops,
                        new_hops = new.hops,
                        "Route changed",
                    );
                }
            }
            _ => {}
        }
    }

    /// Process an incoming Heartbeat message.
    ///
    /// This function:
    /// 1. Records the heartbeat sequence in the routing table
    /// 2. Tracks the forwarding neighbor (adjacency)
    /// 3. Detects and handles duplicate sequences
    /// 4. Discovers new routes or detects route changes
    /// 5. Forwards the heartbeat (broadcast) and sends a reply
    ///
    /// Returns wire-format messages for forwarding and replying, or None if duplicate.
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
                self.args.obu_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id()) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        let seen_seq = entry.get(&message.id()).is_some();
        let duration = Instant::now().duration_since(self.boot);
        if !seen_seq {
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
        }
        // entry is no longer used after this point; NLL ends the borrow here so
        // entry_from can take its own mutable borrow of self.routes below.

        let entry_from = self
            .routes
            .entry(pkt.from()?)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.obu_params.hello_history,
            )?));

        if entry_from.first().is_some_and(|(x, _)| x > &message.id()) {
            entry_from.clear();
        }

        if entry_from.len() == entry_from.capacity() && entry_from.capacity() > 0 {
            entry_from.swap_remove_index(0);
        }

        // Always ensure we have an adjacency entry for the neighbor that forwarded
        // this heartbeat sequence (pkt.from). Insert if absent for this seq id.
        if !entry_from.contains_key(&message.id()) {
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
        }

        // Track that `pkt.from()` forwarded a heartbeat for `message.source()`
        self.source_neighbors
            .entry(message.source())
            .or_default()
            .insert(pkt.from()?);

        // If we've already seen this heartbeat id for the given source, we've now ensured
        // the adjacency entry for pkt.from(), but we should not forward or reply again.
        // Refresh selection/cached candidates to incorporate the newly observed neighbor.
        if seen_seq {
            let _ = self.select_and_cache_upstream(message.source());
            return Ok(None);
        }

        let new_route = self.get_route_to(Some(message.source()));
        let should_cache = old_route.is_none() && new_route.is_some();
        Self::log_route_change(
            old_route,
            new_route,
            mac,
            message.source(),
            true,
            "Route discovered",
        );
        if should_cache {
            let _sel = self.select_and_cache_upstream(message.source());
        }

        if message.source() != pkt.from()? {
            let new_route_from = self.get_route_to(Some(pkt.from()?));
            Self::log_route_change(
                old_route_from,
                new_route_from,
                mac,
                pkt.from()?,
                false,
                "route created on heartbeat",
            );
        }

        let broadcast_wire: Vec<u8> = (&Message::new(
            mac,
            [255; 6].into(),
            PacketType::Control(Control::Heartbeat(message.clone())),
        ))
            .into();

        // Use zero-copy reply construction (6.7x faster than traditional)
        let mut reply_wire = Vec::with_capacity(64);
        Message::serialize_heartbeat_reply_into(message, mac, mac, pkt.from()?, &mut reply_wire);

        Ok(Some(vec![
            ReplyType::WireFlat(broadcast_wire),
            ReplyType::WireFlat(reply_wire),
        ]))
    }

    /// Process an incoming HeartbeatReply message.
    ///
    /// This function:
    /// 1. Records latency measurements in downstream observations
    /// 2. Detects routing loops (bail if reply came from upstream)
    /// 3. Prevents immediate bounce-back (skip forward if reply from next hop)
    /// 4. Updates route selection with fresh latency data
    /// 5. Forwards the reply toward the next upstream node
    ///
    /// Returns wire-format message for forwarding, or None if loop detected or would bounce.
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

        // Read the recorded duration and next_upstream immutably so we can
        // decide action without holding a mutable borrow of the routing
        // structures. We'll perform downstream updates in a short mutable
        // scope below.
        let next_upstream_copy = {
            let Some((_, next_upstream, _, _, _)) = source_entries.get(&message.id()) else {
                bail!("no recollection of the next hop for this route");
            };
            *next_upstream
        };

        // Note: avoid forwarding the HeartbeatReply back to the node it came
        // from. If `pkt.from()` equals our recorded `next_upstream`, sending a
        // reply to `next_upstream` would immediately bounce the packet back and
        // can create a forwarding loop. We'll still record downstream
        // observations below, but skip forwarding in that case.

        // Decide action and emit a trace-level log so we can inspect decisions
        // in live runs.
        let pkt_from = pkt.from()?;
        let sender = message.sender();

        // When the seq-specific next_upstream equals the reply sender a direct forward
        // would loop.  Try three alternatives before bailing:
        //
        //  1. Globally-cached upstream (set by select_and_cache_upstream on prior seqs)
        //  2. Any other next_upstream recorded in a different seq entry for this RSU
        //  3. The RSU source itself (forward directly; works whenever obu5 has a link to RSU)
        //
        // This handles the mutual-loop race where two peer OBUs relay the same heartbeat
        // to each other before either receives the RSU's own broadcast, causing each to
        // record the other as next_upstream for that seq.  The race is most common when
        // the direct RSU→obu5 link has packet loss (loss: 0.2 in the example topology),
        // so obu5 falls back to a peer relay and both peers' next_upstream values point at
        // each other.
        let (action, forward_to) = if next_upstream_copy == sender {
            let alt = self
                .cache
                .get_cached_upstream()
                .filter(|&c| c != sender && c != pkt_from)
                .or_else(|| {
                    // Scan all recorded seq entries for this RSU for any non-looping upstream.
                    source_entries.values().find_map(|(_, mac, _, _, _)| {
                        if *mac != sender && *mac != pkt_from {
                            Some(*mac)
                        } else {
                            None
                        }
                    })
                })
                .or_else(|| {
                    // Last resort: forward directly to the RSU source.
                    let rsu_mac = message.source();
                    if rsu_mac != sender && rsu_mac != pkt_from {
                        Some(rsu_mac)
                    } else {
                        None
                    }
                });

            if let Some(alt_mac) = alt {
                (ForwardAction::ForwardCached, alt_mac)
            } else {
                (ForwardAction::Bail, next_upstream_copy)
            }
        } else if pkt_from == next_upstream_copy {
            (ForwardAction::SkipForward, next_upstream_copy)
        } else {
            (ForwardAction::Forward, next_upstream_copy)
        };

        // Log the decision at appropriate level
        match action {
            ForwardAction::Bail => {} // Will be logged as warn below
            ForwardAction::SkipForward => {
                tracing::debug!(
                    pkt_from = %pkt_from,
                    message_sender = %sender,
                    next_upstream = %next_upstream_copy,
                    "Skipping forward to prevent loop"
                );
            }
            ForwardAction::ForwardCached => {
                tracing::debug!(
                    pkt_from = %pkt_from,
                    message_sender = %sender,
                    next_upstream = %next_upstream_copy,
                    cached_upstream = %forward_to,
                    "seq next_upstream would loop; forwarding via cached upstream"
                );
            }
            ForwardAction::Forward => {
                tracing::trace!(
                    pkt_from = %pkt_from,
                    message_sender = %sender,
                    next_upstream = %next_upstream_copy,
                    "Heartbeat reply forwarding"
                );
            }
        }

        if action == ForwardAction::Bail {
            #[cfg(feature = "stats")]
            node_lib::metrics::inc_loop_detected();
            // Build a compact snapshot of downstream observations for this
            // heartbeat sequence to aid debugging (mac -> observed targets count).
            let downstream_snapshot: Option<Vec<String>> =
                source_entries
                    .get(&message.id())
                    .map(|(_, _nu, _h, _r, downstream)| {
                        downstream
                            .iter()
                            .map(|(mac, vec)| format!("{}:{}", mac, vec.len()))
                            .collect()
                    });

            tracing::warn!(
                pkt_from = %pkt_from,
                message_sender = %sender,
                next_upstream = %next_upstream_copy,
                source = %message.source(),
                seq = message.id(),
                downstream = ?downstream_snapshot,
                "Routing loop detected, dropping packet"
            );
            bail!("loop detected");
        }

        // Update downstream observation lists inside a short mutable scope so
        // we don't hold a mutable borrow across the subsequent `select_and_cache_upstream` call.
        {
            let Some((duration, _next_upstream, _, _, downstream)) =
                source_entries.get_mut(&message.id())
            else {
                bail!("no recollection of the next hop for this route");
            };

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
        }

        // Attempt to select and cache an upstream for the original heartbeat
        // source now that we've recorded downstream observations. Do this
        // before the early-return below so replies that would be skipped for
        // forwarding still cause caching.
        let _selected = self.select_and_cache_upstream(message.source());

        // If the reply arrived from the node we'd forward to, don't forward
        // it back: that would produce an immediate bounce. Drop forwarding
        // (but keep the recorded downstream information above).
        // Exception: when we're using a cached upstream fallback (ForwardCached),
        // pkt_from == next_upstream_copy is intentional — we still forward to forward_to.
        if action == ForwardAction::SkipForward {
            return Ok(None);
        }

        let sender = message.sender();

        // Use flat serialization for better performance (8.7x faster)
        let wire: Vec<u8> = (&Message::new(
            mac,
            forward_to,
            PacketType::Control(Control::HeartbeatReply(message.clone())),
        ))
            .into();

        let reply = Ok(Some(vec![ReplyType::WireFlat(wire)]));

        let new_route = self.get_route_to(Some(sender));
        // Downstream OBU route updates are debug-level; the important INFO event
        // is "Upstream selected/changed" emitted by select_and_cache_upstream.
        Self::log_route_change(old_route, new_route, mac, sender, false, "Route discovered");

        if sender != pkt.from()? {
            let new_route_from = self.get_route_to(Some(pkt.from()?));
            Self::log_route_change(
                old_route_from,
                new_route_from,
                mac,
                pkt.from()?,
                false,
                "route created on heartbeat reply",
            );
        }

        reply
    }

    /// Find the best route to a target MAC address with hysteresis.
    ///
    /// Selection algorithm:
    /// 1. **For cached upstream (None)**: Returns current cached upstream if set
    /// 2. **For non-RSU targets**: Uses downstream observations across all heartbeats
    /// 3. **For RSU targets**: Applies latency-based scoring with hysteresis:
    ///    - Prefers lower average latency (avg scoring)
    ///    - Applies 30% improvement threshold to prevent flapping
    ///    - Falls back to hop-count when latency unavailable
    ///    - Deterministic tie-breaking by MAC address
    ///
    /// Hysteresis: Only switches from cached route when:
    /// - New route has ≥1 fewer hops, OR
    /// - New route has ≥30% better average latency score
    ///
    /// Returns None if no route exists.
    pub fn get_route_to(&self, mac: Option<MacAddress>) -> Option<Route> {
        let Some(target_mac) = mac else {
            // Use the RSU source MAC (cached_source) to compute the upstream route.
            //
            // Using cached_upstream (the next-hop) directly would re-enter
            // get_route_to with the relay OBU's MAC, which is not an RSU source
            // and triggers the downstream lookup — which fails for nodes that only
            // receive RSU heartbeats via a relay (multi-hop nodes like obu5).
            // Using cached_source (RSU MAC) reaches the RSU-keyed branch and
            // returns the correct Route{mac=relay_obu, hops=N} for data forwarding.
            if let Some(source_mac) = self.cache.get_cached_source() {
                return self.get_route_to(Some(source_mac));
            }
            return None;
        };
        // If the target_mac is not an RSU we've recorded heartbeats for, attempt to
        // compute a route toward this node using downstream observations across all
        // heartbeat sequences. This allows forwarding downstream frames toward other
        // OBUs (e.g., two-hop paths) using observed neighbors and latencies.
        if !self.routes.contains_key(&target_mac) {
            // Collect candidate next hops that lead to target_mac along with hop-count and latency.
            let mut candidates: Vec<(u32, MacAddress, u128)> = Vec::new();
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, (_dur, _next_upstream, _hops, _r, downstream)) in seqs.iter() {
                    if let Some(vec) = downstream.get(&target_mac) {
                        for t in vec.iter() {
                            let us = t.latency.map(|d| d.as_micros()).unwrap_or(u128::MAX);
                            candidates.push((t.hops, t.mac, us));
                        }
                    }
                }
            }
            if candidates.is_empty() {
                return None;
            }
            let min_hops = candidates
                .iter()
                .map(|(h, _, _)| *h)
                .min()
                .expect("candidates is non-empty, min must exist");
            use crate::control::routing_utils::{pick_best_next_hop, NextHopStats};

            let mut per_next: std::collections::HashMap<MacAddress, NextHopStats> =
                std::collections::HashMap::new();
            for (_h, mac, us) in candidates.into_iter().filter(|(h, _, _)| *h == min_hops) {
                let e = per_next.entry(mac).or_insert(NextHopStats {
                    min_us: u128::MAX,
                    sum_us: 0,
                    count: 0,
                });
                if us < e.min_us {
                    e.min_us = us;
                }
                if us != u128::MAX {
                    e.sum_us += us;
                    e.count += 1;
                }
            }

            let (mac, avg) = pick_best_next_hop(per_next)?;
            return Some(Route {
                hops: min_hops,
                mac,
                latency: if avg == u128::MAX {
                    None
                } else {
                    Some(Duration::from_micros(avg as u64))
                },
            });
        }
        // Optionally incorporate hysteresis against the currently cached upstream.
        // We will compute the usual "best" candidate, but if it differs from the
        // cached upstream we only switch when it's better by a margin (>=30% lower
        // average latency) or uses at least one fewer hop. Otherwise, we keep the
        // current next hop to avoid flapping.
        let cached = self.get_cached_upstream();
        let mut upstream_routes: Vec<_> = self
            .routes
            .iter()
            .flat_map(|(rsu_mac, seqs)| {
                seqs.iter()
                    .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
            })
            .filter(|(_, rsu_mac, _, _)| rsu_mac == &&target_mac)
            .collect();
        upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));

        // Compute deterministic integer-based metrics for latency in microseconds across ALL hops.
        // Prefer lower latency first; break ties by fewer hops.
        // Build latency_candidates deterministically by scanning all recorded sequences
        // (same approach as `select_and_cache_upstream`) to avoid timing/order issues.
        let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
            HashMap::default();
        for (_rsu, seqs) in self.routes.iter() {
            for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                if let Some(vec) = downstream.get(&target_mac) {
                    for route in vec.iter() {
                        if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                            let entry = latency_candidates.entry(route.mac).or_insert((
                                u128::MAX,
                                0u128,
                                0u32,
                                route.hops,
                            ));
                            if entry.0 > lat {
                                entry.0 = lat;
                            }
                            entry.1 += lat;
                            entry.2 += 1;
                            entry.3 = route.hops;
                        }
                    }
                }
            }
        }

        if !latency_candidates.is_empty() {
            // Use helper to pick the best candidate; clone the map so we can still
            // inspect it below for cached membership/hops.
            let (best_mac, best_avg) =
                crate::control::routing_utils::pick_best_from_latency_candidates(
                    latency_candidates.clone(),
                )
                .expect("latency_candidates non-empty");
            let (best_min, _best_sum, _best_n, best_hops) = latency_candidates
                .get(&best_mac)
                .copied()
                .expect("best_mac must exist in latency_candidates");
            // Score = avg only; avoids the "sticky minimum" problem where one lucky
            // fading burst to a far RSU would permanently bias the score.
            let _ = best_min; // kept in destructuring for clarity but not used in score
            let best_score = best_avg;

            // If cached is set but isn't in latency candidates (no latency observed yet),
            // prefer a measured candidate when available. The previous behavior kept
            // cached unless the best had at least one fewer hop; that prevented
            // switching when the new candidate had strictly better latency but the
            // cached one had no latency measurements. Here we switch to the best
            // measured candidate (when one exists). If there are no measured
            // candidates, fall back to the hops-only hysteresis.
            if let Some(cached_mac) = cached {
                if !latency_candidates.contains_key(&cached_mac) {
                    // If we have a finite scored best (i.e., measured candidate),
                    // prefer it (allow switching). Otherwise fall back to hops-only
                    // decision as before.
                    if best_score != u128::MAX {
                        // best candidate is measured; let the default return of
                        // best happen (do nothing here).
                    } else if let Some((_, _, _, cached_hops_ref)) = upstream_routes
                        .iter()
                        .find(|(_, _, mac_ref, _)| **mac_ref == cached_mac)
                    {
                        let cached_hops = **cached_hops_ref;
                        if best_mac != cached_mac {
                            let fewer_hops = best_hops < cached_hops;
                            if !fewer_hops {
                                return Some(Route {
                                    hops: cached_hops,
                                    mac: cached_mac,
                                    latency: None,
                                });
                            }
                        }
                    }
                }
            }

            // If we have a cached upstream that is also a candidate for this RSU,
            // apply hysteresis: stick to cached unless the new one is clearly better.
            if let Some(cached_mac) = cached {
                if let Some((_cached_min, cached_sum, cached_n, cached_hops)) =
                    latency_candidates.get(&cached_mac).copied()
                {
                    let cached_avg = if cached_n > 0 {
                        cached_sum / (cached_n as u128)
                    } else {
                        u128::MAX
                    };
                    // Score = avg only; symmetric with best_score above.
                    let cached_score = cached_avg;

                    // If best is the cached, just return it.
                    if best_mac == cached_mac {
                        return Some(Route {
                            hops: best_hops,
                            mac: best_mac,
                            latency: if best_avg == u128::MAX {
                                None
                            } else {
                                Some(Duration::from_micros(best_avg as u64))
                            },
                        });
                    }

                    // Switching conditions:
                    // - strictly fewer hops by at least 1
                    // - or average latency better by >=30%
                    let fewer_hops = best_hops < cached_hops;
                    let latency_better_enough =
                        if cached_score == u128::MAX && best_score != u128::MAX {
                            true // prefer finite measurement over unknown
                        } else if cached_score == u128::MAX || best_score == u128::MAX {
                            false
                        } else {
                            // new_avg <= cached_avg * 0.7 (30% or more better)
                            best_score.saturating_mul(10) < cached_score.saturating_mul(7)
                        };

                    if !(fewer_hops || latency_better_enough) {
                        // Keep cached
                        return Some(Route {
                            hops: cached_hops,
                            mac: cached_mac,
                            latency: if cached_avg == u128::MAX {
                                None
                            } else {
                                Some(Duration::from_micros(cached_avg as u64))
                            },
                        });
                    }
                }
            }

            // Default: return the best candidate
            return Some(Route {
                hops: best_hops,
                mac: best_mac,
                latency: if best_avg == u128::MAX {
                    None
                } else {
                    Some(Duration::from_micros(best_avg as u64))
                },
            });
        }

        // Fallback: no latency observed yet, prefer fewer hops (original behavior)
        if let Some((_, _, best_mac_ref, best_hops_ref)) = upstream_routes.first() {
            let best_mac = **best_mac_ref;
            let best_hops = **best_hops_ref;

            // Apply hysteresis with hops-only info when we don't have latency.
            if let Some(cached_mac) = cached {
                if let Some((_, _, _, cached_hops_ref)) = upstream_routes
                    .iter()
                    .find(|(_, _, mac_ref, _)| **mac_ref == cached_mac)
                {
                    let cached_hops = **cached_hops_ref;
                    if best_mac != cached_mac {
                        let fewer_hops = best_hops < cached_hops; // switch only if at least one fewer hop
                        if !fewer_hops {
                            return Some(Route {
                                hops: cached_hops,
                                mac: cached_mac,
                                latency: None,
                            });
                        }
                    }
                }
            }

            return Some(Route {
                hops: best_hops,
                mac: best_mac,
                latency: None,
            });
        }
        None
    }

    /// Reception quality for a given RSU source MAC: fraction of expected heartbeat
    /// sequences that were actually received within the `hello_history` window.
    ///
    /// Returns:
    /// - `1.0`  — all sequences received (great signal, nearby RSU)
    /// - `0.0`  — no data, or the last heartbeat is stale (RSU out of range / gone)
    /// - `0.0–1.0` — partial reception (lossy channel or RSU at range edge)
    ///
    /// Staleness: the heartbeat period is estimated from the observed inter-arrival
    /// times.  If no heartbeat has been received within 3× that period the RSU is
    /// considered stale and 0.0 is returned regardless of historic fill-rate.
    fn rsu_reception_quality(&self, rsu_mac: MacAddress) -> f64 {
        let seqs = match self.routes.get(&rsu_mac) {
            Some(s) if !s.is_empty() => s,
            _ => return 0.0,
        };

        // Estimate inter-heartbeat period from wall-clock reception times stored
        // in the first element of each PerHopInfo tuple.
        let durations: Vec<Duration> = seqs.values().map(|(d, _, _, _, _)| *d).collect();
        let last_recv = *durations.last().unwrap_or(&Duration::ZERO);
        let now = Instant::now().duration_since(self.boot);

        let estimated_period = if durations.len() >= 2 {
            let span = *durations.last().unwrap() - *durations.first().unwrap();
            span / (durations.len() as u32).saturating_sub(1).max(1)
        } else {
            Duration::from_secs(5) // safe default: assume 5 s heartbeat period
        };

        // Treat RSU as gone if its last heartbeat is >3× the estimated period old.
        if now > last_recv + estimated_period * 3 {
            return 0.0;
        }

        if seqs.len() < 2 {
            return 0.5; // insufficient history to compute a meaningful ratio
        }

        let oldest_seq = *seqs.keys().next().unwrap();
        let newest_seq = *seqs.keys().last().unwrap();
        // wrapping_sub handles the (unlikely) u32 rollover case
        let span = newest_seq.wrapping_sub(oldest_seq).saturating_add(1) as f64;
        seqs.len() as f64 / span
    }

    /// Select the best route to an RSU and cache N-best candidates for failover.
    ///
    /// This function:
    /// 1. Applies a signal-quality guard to prevent unnecessary RSU handoffs:
    ///    - With RSSI table: switch only when incoming RSU is >3 dB stronger
    ///      (≈40% closer), or the cached RSU has gone stale (RSSI not available).
    ///    - Without RSSI table: falls back to reception-quality ratio (≥30% better).
    /// 2. Calls `get_route_to()` to find the best route with hysteresis
    /// 3. Caches the selected upstream as primary
    /// 4. Builds an ordered list of N-best candidates for fast failover
    /// 5. Logs when first upstream is selected (important OBU milestone)
    ///
    /// Returns the selected route, or None if no route exists.
    pub fn select_and_cache_upstream(&self, mac: MacAddress) -> Option<Route> {
        // --- Signal-quality guard -------------------------------------------
        // The latency-based path in get_route_to never fires for RSU targets
        // (RSUs do not send heartbeat replies, so they never appear in the
        // downstream observation map).  Without this guard every incoming RSU
        // heartbeat would overwrite the cache with that RSU — causing rapid
        // flapping between all hearable RSUs.
        //
        // When an RSSI table is available (simulator or real radio driver) we use
        // signal strength in dBm as the primary quality metric: a closer RSU has
        // a higher (less negative) RSSI.  We require the incoming RSU to be at
        // least 3 dB stronger before switching — this corresponds to roughly 40%
        // closer distance and is below the threshold of perceptible link degradation.
        //
        // Without RSSI we fall back to heartbeat reception ratio with a 30% margin.
        if let Some(cached_source) = self.cache.get_cached_source() {
            if cached_source != mac {
                let keep_cached = if let Some(ref rssi_tbl) = self.rssi_table {
                    let tbl = rssi_tbl.read().expect("rssi table lock");
                    let rssi_incoming = *tbl.get(&mac).unwrap_or(&-100.0_f32);
                    let rssi_cached = *tbl.get(&cached_source).unwrap_or(&-100.0_f32);
                    // -95 dBm is near the edge of usable range (~3 km free-space at
                    // 5.9 GHz with 23 dBm TX).  Below that the cached RSU is effectively
                    // gone, so we let the guard fall through.
                    rssi_cached > -95.0 && rssi_incoming <= rssi_cached + 3.0
                } else {
                    let q_incoming = self.rsu_reception_quality(mac);
                    let q_cached = self.rsu_reception_quality(cached_source);
                    // Keep cached unless the new RSU is clearly better.
                    // `q_cached > 0.0` — if cached has gone stale we fall through
                    // and let get_route_to pick whatever is reachable.
                    q_cached > 0.0 && q_incoming <= q_cached * 1.3
                };

                if keep_cached {
                    if let Some(cached_up) = self.cache.get_cached_upstream() {
                        return Some(Route {
                            mac: cached_up,
                            hops: 1, // approximate; callers ignore the return value
                            latency: None,
                        });
                    }
                }
            }
        }
        // -----------------------------------------------------------------------

        let route = self.get_route_to(Some(mac))?;
        let old_upstream = self.cache.get_cached_upstream();

        // Store primary cached upstream and source
        self.cache.set_upstream(route.mac, mac);

        match old_upstream {
            None => {
                tracing::info!(
                    upstream = %route.mac,
                    source = %mac,
                    hops = route.hops,
                    "Upstream selected"
                );
            }
            Some(old_mac) if old_mac != route.mac => {
                tracing::info!(
                    upstream = %route.mac,
                    was_upstream = %old_mac,
                    source = %mac,
                    hops = route.hops,
                    "Upstream changed"
                );
            }
            _ => {}
        }
        // Also attempt to populate an ordered list of N-best candidates for fast failover.
        let n_best = self.cache.candidates_count();
        if let Some(candidates) = {
            // Re-run a variant of selection to fetch multiple candidates.
            // Call get_route_to for this mac to trigger same computation; then
            // fall back to computing from latency_candidates in the routing
            // internal structures. Because `get_route_to` is pure, we can
            // compute candidates deterministically by copying the logic here.
            // For simplicity, compute from latency and hops collected across
            // observed routes.
            // Recreate the latency_candidates map used by get_route_to.
            let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
                HashMap::default();
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                    if let Some(vec) = downstream.get(&mac) {
                        for route in vec.iter() {
                            if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                                let entry = latency_candidates.entry(route.mac).or_insert((
                                    u128::MAX,
                                    0u128,
                                    0u32,
                                    route.hops,
                                ));
                                if entry.0 > lat {
                                    entry.0 = lat;
                                }
                                entry.1 += lat;
                                entry.2 += 1;
                                entry.3 = route.hops;
                            }
                        }
                    }
                }
            }
            if !latency_candidates.is_empty() {
                let scored_full = crate::control::routing_utils::score_and_sort_latency_candidates(
                    latency_candidates,
                );
                let mut out: Vec<MacAddress> = scored_full
                    .into_iter()
                    .map(|(_score, _hops, mac, _avg)| mac)
                    .take(n_best)
                    .collect();
                // If we still have capacity, backfill with hop-based candidates not already present
                if out.len() < n_best {
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
                    let mut seen: std::collections::HashSet<MacAddress> =
                        out.iter().copied().collect();
                    for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                        if !seen.contains(mac_ref) {
                            seen.insert(*mac_ref);
                            out.push(*mac_ref);
                            if out.len() >= n_best {
                                break;
                            }
                        }
                    }
                }
                // As a final fallback, add any recorded neighbors that forwarded heartbeats
                // for this source (not yet included), then, if capacity remains, include the
                // source itself.
                if out.len() < n_best {
                    if let Some(neigh) = self.source_neighbors.get(&mac) {
                        for cand in neigh.iter() {
                            if !out.contains(cand) {
                                out.push(*cand);
                                if out.len() >= n_best {
                                    break;
                                }
                            }
                        }
                    }
                }
                if out.len() < n_best && !out.contains(&mac) {
                    out.push(mac);
                }
                Some(out)
            } else {
                // Fallback: choose by fewest hops across upstream_routes
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
                let mut seen = std::collections::HashSet::new();
                let mut out = Vec::new();
                for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                    if !seen.contains(mac_ref) {
                        seen.insert(*mac_ref);
                        out.push(*mac_ref);
                        if out.len() >= n_best {
                            break;
                        }
                    }
                }
                if out.len() < n_best {
                    if let Some(neigh) = self.source_neighbors.get(&mac) {
                        for cand in neigh.iter() {
                            if !out.contains(cand) {
                                out.push(*cand);
                                if out.len() >= n_best {
                                    break;
                                }
                            }
                        }
                    }
                }
                if out.len() < n_best && !out.contains(&mac) {
                    out.push(mac);
                }
                if out.is_empty() {
                    None
                } else {
                    Some(out)
                }
            }
        } {
            // store candidates but do NOT override the already-stored primary
            // cached upstream; keep `route.mac` as the primary to preserve
            // hysteresis semantics handled by `get_route_to`.
            self.cache.set_candidates(candidates);
        }
        #[cfg(feature = "stats")]
        node_lib::metrics::inc_cache_select();
        Some(route)
    }
}
