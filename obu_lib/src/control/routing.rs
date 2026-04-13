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
use std::collections::{HashMap, HashSet};
use tokio::time::{Duration, Instant};

use crate::control::routing_utils::NextHopStats;

// ============================================================================
// Type Definitions
// ============================================================================

/// Action to take when forwarding a HeartbeatReply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardAction {
    /// pkt.from == next_upstream — would bounce, skip forwarding.
    SkipForward,
    /// Safe to forward toward next_upstream.
    Forward,
    /// next_upstream == sender (would loop), forwarding via alternative upstream.
    ForwardCached,
}

/// Maximum number of observations kept per sender MAC in a `SequenceEntry`'s
/// downstream map.
///
/// In a dense mesh a single heartbeat reply from one OBU can arrive at a relay
/// OBU multiple times — once per independent relay path.  Without a cap, the
/// `Vec<Target>` for each sender grows without bound, causing the memory spike
/// observed in the Porto stress scenario (48 RSUs × 70 OBUs).  Keeping the
/// last `MAX_DOWNSTREAM_OBS` observations per sender is sufficient for accurate
/// latency statistics while capping memory to a predictable constant.
const MAX_DOWNSTREAM_OBS: usize = 8;

/// One heartbeat sequence's routing state.
///
/// Records when we received a particular sequence, which neighbor forwarded
/// it, the hop count from the source, and any downstream latency observations
/// collected from heartbeat replies correlated to this sequence.
#[derive(Debug)]
struct SequenceEntry {
    /// Wall-clock time (relative to node boot) when this sequence arrived.
    received_at: Duration,
    /// MAC of the neighbor that forwarded this heartbeat sequence to us.
    next_upstream: MacAddress,
    /// Hop count from the heartbeat source to this node for this sequence.
    hops: u32,
    /// Per-sender latency observations: sender MAC → list of reply targets.
    downstream: HashMap<MacAddress, Vec<Target>>,
}

impl SequenceEntry {
    fn new(received_at: Duration, next_upstream: MacAddress, hops: u32) -> Self {
        Self {
            received_at,
            next_upstream,
            hops,
            downstream: HashMap::default(),
        }
    }

    /// Record a downstream latency observation for a HeartbeatReply.
    ///
    /// Appends two `Target` entries:
    /// - One for `sender` (the OBU that originated the reply), carrying the
    ///   measured round-trip `latency`.
    /// - One for `forwarder` (the immediate peer that delivered the reply),
    ///   with no latency measurement (adjacency-only).
    ///
    /// Each per-sender `Vec<Target>` is capped at `MAX_DOWNSTREAM_OBS` entries
    /// (sliding window — oldest evicted first) to bound memory in dense mesh
    /// scenarios where the same reply can arrive via multiple relay paths.
    fn record_observation(
        &mut self,
        sender: MacAddress,
        sender_hops: u32,
        forwarder: MacAddress,
        latency: Duration,
    ) {
        let sv = self.downstream.entry(sender).or_default();
        if sv.len() >= MAX_DOWNSTREAM_OBS {
            sv.remove(0);
        }
        sv.push(Target {
            hops: sender_hops,
            mac: forwarder,
            latency: Some(latency),
        });

        let fv = self.downstream.entry(forwarder).or_default();
        if fv.len() >= MAX_DOWNSTREAM_OBS {
            fv.remove(0);
        }
        fv.push(Target {
            hops: 1,
            mac: forwarder,
            latency: None,
        });
    }
}

/// Bounded, insertion-ordered heartbeat history for one source node.
///
/// Wraps an `IndexMap<seq_id, SequenceEntry>` with capacity-bounded insertion
/// semantics: [`observe`](SourceHistory::observe) handles rollback detection,
/// oldest-entry eviction, and deduplication in one call.
///
/// Implements `Deref<Target = IndexMap<u32, SequenceEntry>>` so all read-path
/// callers (`iter`, `values`, `get`, `len`, …) work transparently.
struct SourceHistory {
    seqs: IndexMap<u32, SequenceEntry>,
}

impl SourceHistory {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            seqs: IndexMap::with_capacity(capacity),
        }
    }

    /// Record sequence `seq_id` with `entry`.
    ///
    /// - **Rollback**: if `seq_id` < the oldest stored sequence, clears history
    ///   (the RSU restarted its counter).
    /// - **Eviction**: when at capacity, removes the oldest entry to make room.
    /// - **Dedup**: if `seq_id` is already present, skips insertion.
    ///
    /// Returns `true` when the sequence was already recorded (duplicate).
    fn observe(&mut self, seq_id: u32, entry: SequenceEntry) -> bool {
        if self.seqs.first().is_some_and(|(x, _)| x > &seq_id) {
            self.seqs.clear();
        }
        if self.seqs.len() == self.seqs.capacity() && self.seqs.capacity() > 0 {
            self.seqs.swap_remove_index(0);
        }
        if self.seqs.contains_key(&seq_id) {
            return true;
        }
        self.seqs.insert(seq_id, entry);
        false
    }

    /// Insert directly without rollback/eviction logic (test setup only).
    #[cfg(test)]
    fn test_insert(&mut self, seq_id: u32, entry: SequenceEntry) {
        self.seqs.insert(seq_id, entry);
    }
}

impl std::ops::Deref for SourceHistory {
    type Target = IndexMap<u32, SequenceEntry>;
    fn deref(&self) -> &Self::Target {
        &self.seqs
    }
}

impl std::ops::DerefMut for SourceHistory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.seqs
    }
}

/// Routing table: source MAC → bounded heartbeat sequence history.
type RoutingTable = HashMap<MacAddress, SourceHistory>;

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

/// Per-relay-hop RSSI penalty applied when comparing next-hop candidates.
///
/// Each relay hop adds processing overhead, queue pressure, and an additional
/// failure point. This penalty discounts the measured first-hop RSSI by 3 dB
/// per relay so that longer chains must present a proportionally stronger
/// signal to be preferred. 5 dB corresponds to roughly 78% closer in
/// free-space path loss (20·log₁₀ model at 5.9 GHz).
const RSSI_HOP_PENALTY_DB: f32 = 5.0;

// ============================================================================
// Route construction helpers
// ============================================================================

/// Compute average latency in µs from aggregated stats, or `u128::MAX` if
/// there are no measurements.
fn stats_avg(s: &NextHopStats) -> u128 {
    if s.count > 0 {
        s.sum_us / (s.count as u128)
    } else {
        u128::MAX
    }
}

/// Convert a raw µs average to an `Option<Duration>` (`None` for `u128::MAX`).
fn finite_duration(avg_us: u128) -> Option<Duration> {
    if avg_us == u128::MAX {
        None
    } else {
        Some(Duration::from_micros(avg_us as u64))
    }
}

/// Build a `Route` from the standard (mac, hops, avg_us) triple.
fn make_route(mac: MacAddress, hops: u32, avg_us: u128) -> Route {
    Route {
        mac,
        hops,
        latency: finite_duration(avg_us),
    }
}

/// Return `true` when `new_avg` is significantly better than `cached_avg`
/// (at least 30% lower, or any finite measurement beats an unmeasured cached).
fn is_significantly_better(new_avg: u128, cached_avg: u128) -> bool {
    if cached_avg == u128::MAX && new_avg != u128::MAX {
        true // prefer any measurement over none
    } else if cached_avg == u128::MAX || new_avg == u128::MAX {
        false
    } else {
        // new_avg <= cached_avg * 0.7
        new_avg.saturating_mul(10) < cached_avg.saturating_mul(7)
    }
}

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

    /// Build an ordered list of up to `n_best` candidate next-hops for `src`.
    ///
    /// Priority order:
    /// 1. Latency-scored candidates (measured, best avg first)
    /// 2. Hop-count-ordered candidates (unmeasured backfill)
    /// 3. Known source neighbors
    /// 4. The source itself as a last resort
    fn build_candidate_list(&self, src: MacAddress, n_best: usize) -> Vec<MacAddress> {
        let mut out: Vec<MacAddress> = Vec::with_capacity(n_best);

        let latency_stats = self.collect_downstream_stats(src);
        if !latency_stats.is_empty() {
            let scored =
                crate::control::routing_utils::score_and_sort_latency_candidates(&latency_stats);
            for c in scored {
                if out.len() >= n_best {
                    break;
                }
                out.push(c.mac);
            }
        }

        // Backfill with next-hops not already present, ordered by RSSI descending
        // when the table is available, or by hops ascending otherwise.
        if out.len() < n_best {
            // Snapshot the RSSI table so the lock is not held during sorting.
            let rssi_snap: HashMap<MacAddress, f32> = self
                .rssi_table
                .as_ref()
                .map(|tbl| tbl.read().expect("rssi table lock").clone())
                .unwrap_or_default();

            let mut hop_routes: Vec<_> = self
                .routes
                .iter()
                .flat_map(|(rsu_mac, seqs)| {
                    seqs.iter()
                        .map(move |(_, e)| (rsu_mac, &e.next_upstream, &e.hops))
                })
                .filter(|(rsu_mac, _, _)| *rsu_mac == &src)
                .collect();
            if rssi_snap.is_empty() {
                hop_routes.sort_by_key(|(_, _, hops)| *hops);
            } else {
                // Sort by effective RSSI descending: penalise relay chains so that
                // a longer path must have a proportionally stronger first-hop signal.
                hop_routes.sort_by(|(_, mac_a, hops_a), (_, mac_b, hops_b)| {
                    let ra = rssi_snap.get(*mac_a).copied().unwrap_or(-100.0_f32)
                        - RSSI_HOP_PENALTY_DB * (*hops_a).saturating_sub(1) as f32;
                    let rb = rssi_snap.get(*mac_b).copied().unwrap_or(-100.0_f32)
                        - RSSI_HOP_PENALTY_DB * (*hops_b).saturating_sub(1) as f32;
                    rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            let mut seen: HashSet<MacAddress> = out.iter().copied().collect();
            for (_rsu, mac, _hops) in hop_routes {
                if seen.insert(*mac) {
                    out.push(*mac);
                    if out.len() >= n_best {
                        break;
                    }
                }
            }
        }

        // Add known neighbors that forwarded heartbeats for this source.
        if out.len() < n_best {
            if let Some(neigh) = self.source_neighbors.get(&src) {
                for &cand in neigh.iter() {
                    if !out.contains(&cand) {
                        out.push(cand);
                        if out.len() >= n_best {
                            break;
                        }
                    }
                }
            }
        }

        // Last resort: the source itself.
        if out.len() < n_best && !out.contains(&src) {
            out.push(src);
        }

        out
    }

    /// Aggregate downstream observations for `target` across all recorded
    /// heartbeat sequences, returning per-next-hop latency statistics.
    ///
    /// Only observations that carry a latency measurement are included.
    fn collect_downstream_stats(&self, target: MacAddress) -> HashMap<MacAddress, NextHopStats> {
        let mut stats: HashMap<MacAddress, NextHopStats> = HashMap::with_capacity(4);
        for (_rsu, seqs) in self.routes.iter() {
            for (_seq, e) in seqs.iter() {
                if let Some(vec) = e.downstream.get(&target) {
                    for route in vec.iter() {
                        if let Some(lat_us) = route.latency.map(|d| d.as_micros()) {
                            let e = stats.entry(route.mac).or_insert(NextHopStats {
                                min_us: u128::MAX,
                                sum_us: 0,
                                count: 0,
                                hops: route.hops,
                            });
                            if lat_us < e.min_us {
                                e.min_us = lat_us;
                            }
                            e.sum_us += lat_us;
                            e.count += 1;
                            e.hops = route.hops;
                        }
                    }
                }
            }
        }
        stats
    }

    /// Rotate to the next cached candidate (promote the next candidate to primary).
    /// Returns the newly promoted primary if any.
    pub fn failover_cached_upstream(&self) -> Option<MacAddress> {
        self.cache
            .failover(|src, n_best| self.build_candidate_list(src, n_best))
    }

    /// Test helper: directly set cached candidates and primary for tests.
    #[cfg(test)]
    pub fn test_set_cached_candidates(&self, cands: Vec<MacAddress>) {
        self.cache.test_set_cached_candidates(cands);
    }

    /// Test helper: directly set cached upstream and source for tests.
    #[cfg(test)]
    pub fn test_set_cached_upstream(&self, upstream: MacAddress, source: MacAddress) {
        self.cache.set_upstream(upstream, source);
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

        let capacity = usize::try_from(self.args.obu_params.hello_history)?;
        let duration = Instant::now().duration_since(self.boot);

        // Record this sequence for the heartbeat source; learn if it's a duplicate.
        let seen_seq = self
            .routes
            .entry(message.source())
            .or_insert_with(|| SourceHistory::with_capacity(capacity))
            .observe(
                message.id(),
                SequenceEntry::new(duration, pkt.from()?, message.hops()),
            );

        // Always record an adjacency entry for the neighbor that forwarded this
        // heartbeat (pkt.from), even when the source seq is a duplicate.
        self.routes
            .entry(pkt.from()?)
            .or_insert_with(|| SourceHistory::with_capacity(capacity))
            .observe(message.id(), SequenceEntry::new(duration, pkt.from()?, 1));

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
        // Re-evaluate the cached upstream on every new non-duplicate heartbeat.
        // Restricting this to old_route.is_none() (first discovery) means that
        // when the primary path disappears and only relay heartbeats arrive as new
        // sequences, the cache is never refreshed and the OBU becomes orphaned.
        // The guards inside select_and_cache_upstream prevent unnecessary switching.
        let should_cache = new_route.is_some();
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

    /// Decide how to forward a HeartbeatReply, handling the mutual-loop race.
    ///
    /// Three possible outcomes (returned as `(ForwardAction, forward_to)`):
    /// - `Forward` — normal case, forward to `next_upstream`.
    /// - `SkipForward` — reply arrived from the node we'd send to; drop forwarding
    ///   to prevent an immediate bounce.
    /// - `ForwardCached` — `next_upstream == sender` (loop race); we found an
    ///   alternative upstream to use instead.
    ///
    /// Bails with `"loop detected"` when `next_upstream == sender` and no
    /// safe alternative upstream exists.
    fn decide_reply_forward(
        next_upstream: MacAddress,
        pkt_from: MacAddress,
        sender: MacAddress,
        source: MacAddress,
        seq_id: u32,
        source_history: &SourceHistory,
        cached_upstream: Option<MacAddress>,
    ) -> Result<(ForwardAction, MacAddress)> {
        if next_upstream == sender {
            // Mutual-loop race: two peer OBUs each recorded the other as next_upstream
            // for this seq before either saw the RSU's own broadcast. Try alternatives:
            //  1. Globally-cached upstream (from prior, unaffected seqs)
            //  2. Any other next_upstream from a different seq entry for this RSU
            //  3. The RSU source itself (direct link, if available)
            let alt = cached_upstream
                .filter(|&c| c != sender && c != pkt_from)
                .or_else(|| {
                    source_history.values().find_map(|e| {
                        (e.next_upstream != sender && e.next_upstream != pkt_from)
                            .then_some(e.next_upstream)
                    })
                })
                .or_else(|| (source != sender && source != pkt_from).then_some(source));

            match alt {
                Some(alt_mac) => {
                    tracing::debug!(
                        pkt_from = %pkt_from,
                        sender = %sender,
                        next_upstream = %next_upstream,
                        via = %alt_mac,
                        "seq next_upstream would loop; forwarding via alternative"
                    );
                    Ok((ForwardAction::ForwardCached, alt_mac))
                }
                None => {
                    #[cfg(feature = "stats")]
                    node_lib::metrics::inc_loop_detected();
                    let snapshot = source_history.get(&seq_id).map(|e| {
                        e.downstream
                            .iter()
                            .map(|(mac, v)| format!("{}:{}", mac, v.len()))
                            .collect::<Vec<_>>()
                    });
                    tracing::warn!(
                        pkt_from = %pkt_from,
                        sender = %sender,
                        next_upstream = %next_upstream,
                        source = %source,
                        seq = seq_id,
                        downstream = ?snapshot,
                        "Routing loop detected, dropping packet"
                    );
                    bail!("loop detected");
                }
            }
        } else if pkt_from == next_upstream {
            tracing::debug!(
                pkt_from = %pkt_from,
                sender = %sender,
                next_upstream = %next_upstream,
                "Skipping forward to prevent loop"
            );
            Ok((ForwardAction::SkipForward, next_upstream))
        } else {
            tracing::trace!(
                pkt_from = %pkt_from,
                sender = %sender,
                next_upstream = %next_upstream,
                "Heartbeat reply forwarding"
            );
            Ok((ForwardAction::Forward, next_upstream))
        }
    }

    /// Process an incoming HeartbeatReply message.
    ///
    /// Pipeline:
    /// 1. **Parse** — validate packet type, extract message fields.
    /// 2. **Lookup** — retrieve the recorded `next_upstream` for this seq.
    /// 3. **Decide** — determine forward action via `decide_reply_forward`.
    /// 4. **Observe** — record latency measurement in downstream stats.
    /// 5. **Emit** — serialize forward wire and log route changes.
    ///
    /// Returns wire-format message for forwarding, or None if forwarding is skipped.
    pub fn handle_heartbeat_reply(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::HeartbeatReply(message)) = pkt.get_packet_type() else {
            bail!("this is supposed to be a HeartBeat Reply");
        };

        let pkt_from = pkt.from()?;
        let sender = message.sender();
        let source = message.source();

        let old_route = self.get_route_to(Some(sender));
        let old_route_from = self.get_route_to(Some(pkt_from));

        // Stage 2 — Lookup: retrieve next_upstream and decide forward action.
        // Stage 3 — Observe: record latency measurement.
        // Both stages share `source_history`; drop its borrow before stage 4.
        let (action, forward_to) = {
            let Some(source_history) = self.routes.get_mut(&source) else {
                bail!("we don't know how to reach that source");
            };

            // Lookup: read next_upstream for this seq.
            let next_upstream = {
                let Some(entry) = source_history.get(&message.id()) else {
                    bail!("no recollection of the next hop for this route");
                };
                entry.next_upstream
            };

            // Decide: determine forward action (handles loop detection).
            let cached_upstream = self.cache.get_cached_upstream();
            let (action, forward_to) = Self::decide_reply_forward(
                next_upstream,
                pkt_from,
                sender,
                source,
                message.id(),
                source_history,
                cached_upstream,
            )?;

            // Observe: record the round-trip latency for this sender.
            let Some(entry) = source_history.get_mut(&message.id()) else {
                bail!("no recollection of the next hop for this route");
            };
            let latency = Instant::now().duration_since(self.boot) - entry.received_at;
            entry.record_observation(sender, message.hops(), pkt_from, latency);

            (action, forward_to)
        }; // source_history borrow ends here

        // Stage 4 — Cache: update upstream selection with fresh latency data.
        // Done before the SkipForward early-return so bounced replies still update the cache.
        let _selected = self.select_and_cache_upstream(source);

        if action == ForwardAction::SkipForward {
            return Ok(None);
        }

        // Stage 5 — Emit: serialize the forwarded reply and log route changes.
        // Use flat serialization for better performance (8.7x faster).
        let wire: Vec<u8> = (&Message::new(
            mac,
            forward_to,
            PacketType::Control(Control::HeartbeatReply(message.clone())),
        ))
            .into();

        let reply = Ok(Some(vec![ReplyType::WireFlat(wire)]));

        // Downstream OBU route updates are debug-level; the important INFO event
        // is "Upstream selected/changed" emitted by select_and_cache_upstream.
        let new_route = self.get_route_to(Some(sender));
        Self::log_route_change(old_route, new_route, mac, sender, false, "Route discovered");

        if sender != pkt_from {
            let new_route_from = self.get_route_to(Some(pkt_from));
            Self::log_route_change(
                old_route_from,
                new_route_from,
                mac,
                pkt_from,
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
    ///    - Falls back to RSSI-based next-hop selection (3 dB hysteresis) when
    ///      latency is unavailable and an RSSI table is attached
    ///    - Falls back to hop-count when neither latency nor RSSI is available
    ///    - Deterministic tie-breaking by MAC address
    ///
    /// Hysteresis: Only switches from cached route when:
    /// - New route has ≥1 fewer hops (hop-count path), OR
    /// - New route has ≥30% better average latency score (latency path), OR
    /// - New next-hop has ≥3 dB stronger RSSI (RSSI path)
    ///
    /// Returns None if no route exists.
    /// Sequence entries older than this are considered stale and excluded from
    /// route selection.  Prevents OBUs from routing through relay neighbours
    /// that have moved out of RSU range: without this guard, a relay OBU that
    /// stopped forwarding RSU heartbeats would remain as a cached next-hop
    /// indefinitely, creating a dead-end chain in the routing graph.
    ///
    /// 60 s ≈ 12 × 5 s heartbeat intervals — plenty of margin while still
    /// recovering within one or two RSU hello_history windows.
    const ROUTE_STALE_THRESHOLD: Duration = Duration::from_secs(60);

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
                let route = self.get_route_to(Some(source_mac));
                if route.is_none() {
                    // All sequences for the cached RSU are stale (relay moved away).
                    // Clear the cache so the OBU re-selects on the next heartbeat.
                    self.cache.clear();
                }
                return route;
            }
            return None;
        };
        // If the target_mac is not an RSU we've recorded heartbeats for, attempt to
        // compute a route toward this node using downstream observations across all
        // heartbeat sequences. This allows forwarding downstream frames toward other
        // OBUs (e.g., two-hop paths) using observed neighbors and latencies.
        if !self.routes.contains_key(&target_mac) {
            // Collect candidate next hops that lead to target_mac along with hop-count and latency.
            let mut candidates: Vec<(u32, MacAddress, u128)> = Vec::with_capacity(8);
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, e) in seqs.iter() {
                    if let Some(vec) = e.downstream.get(&target_mac) {
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
            use crate::control::routing_utils::pick_best_next_hop;

            let mut per_next: HashMap<MacAddress, NextHopStats> = HashMap::with_capacity(4);
            for (_h, mac, us) in candidates.into_iter().filter(|(h, _, _)| *h == min_hops) {
                let e = per_next.entry(mac).or_insert(NextHopStats {
                    min_us: u128::MAX,
                    sum_us: 0,
                    count: 0,
                    hops: min_hops,
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
            return Some(make_route(mac, min_hops, avg));
        }
        // Apply hysteresis: prefer the cached upstream unless the best candidate
        // is clearly better (≥1 fewer hop, or ≥30% lower avg latency).
        let cached = self.get_cached_upstream();

        // Build a mac → min-hops map for all relay paths toward target_mac.
        // Used as fallback when a candidate has no latency measurements yet.
        // Only consider sequences received recently; stale entries indicate
        // the relay has moved out of RSU range and is no longer valid.
        let now_dur = Instant::now().duration_since(self.boot);
        let mut upstream_hops: HashMap<MacAddress, u32> = HashMap::with_capacity(4);
        if let Some(seqs) = self.routes.get(&target_mac) {
            for e in seqs.values() {
                if now_dur.saturating_sub(e.received_at) > Self::ROUTE_STALE_THRESHOLD {
                    continue;
                }
                let slot = upstream_hops.entry(e.next_upstream).or_insert(e.hops);
                if e.hops < *slot {
                    *slot = e.hops;
                }
            }
        }

        // All known sequences for this RSU are stale — no valid relay exists.
        if upstream_hops.is_empty() {
            return None;
        }

        let latency_stats = self.collect_downstream_stats(target_mac);

        if !latency_stats.is_empty() {
            let (best_mac, best_avg) =
                crate::control::routing_utils::pick_best_from_latency_candidates(&latency_stats)
                    .expect("latency_stats non-empty");
            let best_hops = latency_stats[&best_mac].hops;

            if let Some(cached_mac) = cached {
                // Early return: already on the best candidate.
                if best_mac == cached_mac {
                    return Some(make_route(best_mac, best_hops, best_avg));
                }

                let cached_in_stats = latency_stats.get(&cached_mac).copied();
                let keep_cached = match cached_in_stats {
                    Some(cs) => {
                        // Both candidates measured: apply latency hysteresis.
                        let cached_avg = stats_avg(&cs);
                        !(best_hops < cs.hops || is_significantly_better(best_avg, cached_avg))
                    }
                    None => {
                        // Cached has no measurements. Switch if best is measured;
                        // otherwise only switch if best has strictly fewer hops.
                        best_avg == u128::MAX
                            && upstream_hops
                                .get(&cached_mac)
                                .is_some_and(|&ch| best_hops >= ch)
                    }
                };

                if keep_cached {
                    let (hops, latency) = match cached_in_stats {
                        Some(cs) => (cs.hops, finite_duration(stats_avg(&cs))),
                        None => (
                            *upstream_hops.get(&cached_mac).expect(
                                "keep_cached=true with no stats implies upstream_hops has entry",
                            ),
                            None,
                        ),
                    };
                    return Some(Route {
                        mac: cached_mac,
                        hops,
                        latency,
                    });
                }
            }

            return Some(make_route(best_mac, best_hops, best_avg));
        }

        // Fallback: no latency measurements yet.
        //
        // If the RSSI table has measurements for next-hop candidates, prefer the
        // one with the strongest first-hop signal (3 dB hysteresis prevents
        // flapping).  Without RSSI, fall back to min-hops with the original
        // hop-count hysteresis.
        let rssi_snapshot: HashMap<MacAddress, f32> = self
            .rssi_table
            .as_ref()
            .map(|tbl| {
                let guard = tbl.read().expect("rssi table lock");
                upstream_hops
                    .keys()
                    .filter_map(|&mac| guard.get(&mac).map(|&r| (mac, r)))
                    .collect()
            })
            .unwrap_or_default();

        if !rssi_snapshot.is_empty() {
            // RSSI path: rank next-hops by effective RSSI, penalising relay
            // chains to account for processing overhead and queue pressure at
            // intermediate nodes.
            //   eff_rssi = raw_rssi - RSSI_HOP_PENALTY_DB × relay_hops
            // where relay_hops = hops - 1 (0 for direct path to RSU).
            let eff_rssi = |mac: MacAddress| -> f32 {
                let raw = rssi_snapshot.get(&mac).copied().unwrap_or(-100.0_f32);
                let relay_hops = upstream_hops
                    .get(&mac)
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1);
                raw - RSSI_HOP_PENALTY_DB * relay_hops as f32
            };

            let best_mac = upstream_hops.keys().copied().max_by(|&a, &b| {
                eff_rssi(a)
                    .partial_cmp(&eff_rssi(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(best_mac) = best_mac {
                let best_hops = upstream_hops[&best_mac];
                if let Some(cached_mac) = cached {
                    if best_mac != cached_mac {
                        if let Some(&cached_hops) = upstream_hops.get(&cached_mac) {
                            // 3 dB hysteresis on effective RSSI: only switch if the
                            // new next-hop is clearly better after the hop penalty.
                            if eff_rssi(best_mac) <= eff_rssi(cached_mac) + 3.0 {
                                return Some(Route {
                                    mac: cached_mac,
                                    hops: cached_hops,
                                    latency: None,
                                });
                            }
                        }
                    }
                }
                return Some(Route {
                    mac: best_mac,
                    hops: best_hops,
                    latency: None,
                });
            }
        }

        // No RSSI data: fall back to min-hops with the original hop-count hysteresis.
        let best_hop = upstream_hops.iter().min_by_key(|(_, &h)| h);
        if let Some((&best_mac, &best_hops)) = best_hop {
            if let Some(cached_mac) = cached {
                if best_mac != cached_mac {
                    if let Some(&cached_hops) = upstream_hops.get(&cached_mac) {
                        if best_hops >= cached_hops {
                            return Some(Route {
                                mac: cached_mac,
                                hops: cached_hops,
                                latency: None,
                            });
                        }
                    }
                }
            }
            return Some(Route {
                mac: best_mac,
                hops: best_hops,
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
        // in each SequenceEntry.
        let durations: Vec<Duration> = seqs.values().map(|e| e.received_at).collect();
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
                    // Distinguish "not yet measured" from "measured as very weak".
                    // If the cached RSU has no entry (table empty at startup, before
                    // the fading task has run), keep it rather than treating it as
                    // gone — the -100 dBm fallback would fail the -95 dBm liveness
                    // check and cause every arriving heartbeat to evict the cached RSU.
                    match tbl.get(&cached_source) {
                        None => {
                            // Two cases:
                            // 1. Table is empty → startup, no measurements yet → keep.
                            // 2. Table has entries but not this RSU → fading task removed
                            //    it because it went out of range → allow switching.
                            tbl.is_empty()
                        }
                        Some(&rssi_cached) => {
                            // -95 dBm is near the edge of usable range (~3 km free-space
                            // at 5.9 GHz with 23 dBm TX).  Below that the cached RSU is
                            // effectively gone, so we let the guard fall through.
                            rssi_cached > -95.0 && rssi_incoming <= rssi_cached + 3.0
                        }
                    }
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
        // Build an ordered list of N-best candidates for fast failover.
        let n_best = self.cache.candidates_count();
        let candidates = self.build_candidate_list(mac, n_best);
        if !candidates.is_empty() {
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
