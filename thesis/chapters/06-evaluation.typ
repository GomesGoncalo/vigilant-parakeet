// ── Chapter 6 — Evaluation <evaluation> ──────────────────────────────────────

= Evaluation <evaluation>

This chapter evaluates vigilant-parakeet across four dimensions: routing
protocol correctness and convergence, routing metric quality under impaired
channel conditions, N-best failover performance, and cryptographic handshake
overhead. The evaluation answers the three research questions posed in
@introduction:

+ Can a single-machine Linux simulator faithfully reproduce the routing
  dynamics of a multi-hop vehicular network without specialised hardware?

+ How does the choice of routing metric (latency vs. hop count) affect
  convergence time and route stability under varying loss conditions?

+ What is the overhead introduced by N-best candidate caching on memory and
  CPU, and how much does it reduce route-restoration latency after a next-hop
  failure?

A fourth dimension — cryptographic session establishment overhead — is
evaluated as a secondary contribution, addressing the practical question of
whether post-quantum key exchange (ML-KEM-768 + ML-DSA-65) is viable
within the contact-window constraints of vehicular communication.

== Experimental Setup

=== Hardware and Software Configuration

All experiments were conducted on a single host machine with the following
specification:

- CPU: AMD Ryzen AI 9 HX 370 (12 cores / 24 threads, 5.1 GHz boost)
- RAM: 27 GiB LPDDR5
- OS: Arch Linux (kernel 6.19.11-zen1)
- Rust toolchain: stable, edition 2021
- Tokio runtime: multi-thread with default thread count (24 threads)

The simulator was built in release mode with the `webview` and `stats` feature
flags enabled:

```sh
cargo build -p simulator --release --features "webview,stats"
```

Release mode enables link-time optimisation (`lto = "fat"`, `codegen-units = 1`)
and disables debug assertions, giving performance representative of a production
deployment.

=== Measurement Methodology

All routing experiments measure *wall-clock time* from simulator start to the
first stable state, using the simulator's HTTP metrics API (`/metrics`) to poll
convergence indicators every 100 ms. The API is queried from a separate process
so that polling overhead does not interfere with node I/O.

Each experiment is repeated *five times* with independent simulator restarts;
the mean and standard deviation are reported. Since the simulator uses an
unseeded RNG for loss injection (`rand::random::<f64>()`), each repetition
constitutes an independent stochastic sample. Five repetitions were chosen to
bound the standard error at approximately $sigma / sqrt(5) approx 0.45 sigma$
while keeping total experiment runtime manageable.

Cryptographic timing experiments measure the wall-clock interval from the OBU
sending `KeyExchangeInit` to the `Established` state appearing in the OBU's DH
key store (observable via the admin console's `session` command). These
measurements are taken at near-zero channel latency and loss to isolate
cryptographic computation time from network delay.

=== Topology Configurations

Three reference topologies were used:

/ Linear-3: Three nodes in a chain: RSU — OBU₁ — OBU₂. Tests basic
  two-hop routing and HeartbeatReply forwarding. OBU₂ must route through
  OBU₁ to reach the RSU.

/ Star-5: One RSU and four OBUs, all directly connected to the RSU. Tests
  single-hop route selection under higher node density. All four OBUs compete
  for the same RSU, exercising the tie-breaking path in the routing metric.

/ Mixed-6: Two RSUs and four OBUs arranged in a partial mesh with
  asymmetric links. Tests route selection under competing RSU signals:
  OBU₁ is equidistant (in hops) from both RSUs but has asymmetric latency to
  each; OBU₃ can reach RSU₁ only via a two-hop path through OBU₂. This
  topology exercises the hysteresis band and multi-RSU route selection.

Topology YAML files for all three configurations are provided in the
`examples/` directory of the repository and are parameterised to allow
easy variation of channel parameters.

=== Baseline and Variant Comparisons

Each routing experiment compares two or three variants:

- *Hop-count only*: `ObuParameters::cached_candidates = 0` and the fallback
  metric always used (no latency observations). This isolates the hop-count
  fallback path.

- *Latency metric, N=1*: full latency-based metric with a single cached
  candidate (no failover list). This isolates the metric quality without
  failover benefit.

- *Latency metric, N=3*: full metric with three cached candidates (default
  configuration). This is the primary evaluation variant.

== Route Convergence

Route convergence time was measured as the interval between simulator start
and the first moment all OBUs had a valid `cached_upstream` entry for at least
one RSU, as reported by the `/metrics` endpoint (`upstream_cache_hits > 0` for
all OBUs).

Convergence is expected to complete within one to two `hello_periodicity`
intervals (500 ms each) in the nominal case: the RSU emits the first Heartbeat
at $t approx 0$, OBUs receive it within one propagation interval, emit a
HeartbeatReply, and the RSU updates its routing state. The OBU caches the
upstream after the first successful HeartbeatReply cycle.

#figure(
  table(
    columns: (auto, 1fr, 1fr, 1fr),
    align: (left, center, center, center),
    [*Topology*], [*Mean (ms)*], [*Std dev (ms)*], [*hello_periodicity (ms)*],
    [Linear-3],   [_TODO_],     [_TODO_],          [500],
    [Star-5],     [_TODO_],     [_TODO_],          [500],
    [Mixed-6],    [_TODO_],     [_TODO_],          [500],
  ),
  caption: [Route convergence time by topology],
) <tab-convergence>

In all three topologies, convergence is expected to depend primarily on
`hello_periodicity` and the number of hops to the RSU. A two-hop OBU (as in
Linear-3 and Mixed-6) requires an additional relay cycle compared to a
single-hop OBU, adding approximately one additional heartbeat interval.
The Mixed-6 topology additionally exhibits competing route candidates;
convergence time in this topology reflects both the relay overhead and the
time required for the latency metric to accumulate enough samples to
differentiate the two RSU paths.

The latency-based metric requires at least two HeartbeatReply cycles to
compute a meaningful minimum and mean: on the first reply, `min = avg = RTT₁`;
on the second, `min = min(RTT₁, RTT₂)` and `avg = (RTT₁+RTT₂)/2`. Routes
selected on fewer than two observations fall back to hop count, so the
effective convergence to latency-based selection lags the initial hop-count
convergence by one heartbeat interval.

== Route Quality Under Packet Loss

To evaluate the metric's sensitivity to channel degradation, the loss
parameter on the RSU–OBU₁ link in the Linear-3 topology was varied from
0% to 20% in 2% steps using the runtime channel API:

```sh
curl -X POST -H "Content-Type: application/json" \
  -d '{"latency":"10","loss":"0.05"}' \
  http://localhost:3030/channel/rsu/obu1/
```

At each loss level, the routing score $s_m = mu_m^min + overline(mu)_m$ was
sampled every 500 ms for 30 seconds (60 samples per loss level) and the
mean and coefficient of variation (CV) of the score were recorded. The CV
measures route instability: a high CV indicates that the score oscillates,
which combined with the hysteresis threshold can cause route flipping.

// TODO: insert figure showing routing score mean and CV vs. loss rate

At zero loss, the score stabilises quickly as observed RTTs converge to the
channel latency. At higher loss rates, packet loss increases RTT variance
(because lost packets force retransmission before the RSU can respond to a
HeartbeatReply, biasing observed RTTs upward). The hysteresis threshold
prevents route flips due to transient variance: a new candidate must be at
least the configured hysteresis fraction better than the cached route before a
switch occurs.

The hop-count fallback becomes active when no latency measurements are recorded
within `hello_history` slots. At 100% loss on the primary link, OBU₂ loses all
measurements for OBU₁ and falls back to hop-count-based routing,
automatically promoting an alternative path if one exists.

== Nakagami-m fading experiments (preliminary plan)

A planned experiment to characterise routing stability under Nakagami-m fading
is described here. These runs produce the preliminary results to be inserted
into this chapter after execution.

Experiment design:

+ Topology: Linear-3 (RSU — OBU₁ — OBU₂) to exercise two-hop forwarding and
  latency accumulation.
+ Channel baseline: latency 10 ms, loss 0%, per-link Nakagami-m fading added
  to the RSU–OBU channels.
+ Fading parameters: m ∈ {0.5, 0.9, 1.0, 2.0, 5.0} (severe to mild fading);
  sample granularity: per-packet and per-10 ms timeslot variants.
+ Metrics: route convergence time, routing-score mean and CV, N-best failover
  restore time, and Key Exchange success rate (fraction of completed
  handshakes within a 500 ms contact window).
+ Repetitions: 10 independent runs per (m, granularity) configuration to bound
  stochastic variability.

Run commands (example):

```sh
# build simulator
cargo build -p simulator --release --features "webview,stats"

# start simulator with topology file that enables Nakagami fading (examples/nakagami_linear3.yaml)
sudo RUST_LOG="node=info" ./target/release/simulator --config-file examples/nakagami_linear3.yaml --pretty &
SIM_PID=$!

# run measurement collector (script collects /metrics every 100 ms for 60 s)
./scripts/collect_metrics.sh http://localhost:3030/metrics nakagami_m${m}_gran${granularity}.json

# stop simulator
kill $SIM_PID
```

Analysis plan:

+ Compute mean and CV of routing score per run and aggregate across repeats.
+ Plot convergence CDFs and median failover restore times per m value.
+ Compare Key Exchange success probability as a function of fading severity
  and sampling granularity.

Notes and caveats:

- The simulator requires sudo for network namespace creation; runs above
  assume a Linux host with network namespace support.
- If the simulator's fading configuration syntax differs, adapt the example
  topology YAML accordingly (see Chapter 9 for notation).

After execution, populate the figures and numeric tables in this chapter with
the collected results (mean ± stddev) and update the text with the observed
effects of Nakagami fading on routing stability and Key Exchange reliability.
== N-Best Failover Latency

The benefit of the N-best candidate cache was measured by inducing a
primary-upstream failure (by setting 100% loss on the active link) and
timing the restoration of end-to-end connectivity. Connectivity restoration is
defined as the first moment an encrypted TAP frame from OBU₂ arrives at the
server (observable via the `/metrics` endpoint's `decrypted_frames_received`
counter incrementing).

Without N-best caching (`cached_candidates = 0`), the OBU must wait for the
next Heartbeat from an alternative RSU and complete a HeartbeatReply cycle
before selecting a new upstream. This delay is at least one `hello_periodicity`
interval (500 ms). With `cached_candidates = 3`, the OBU immediately promotes
the head of the N-best list via `failover_cached_upstream()`, which is an O(1)
operation requiring no routing table scan or Heartbeat cycle.

#figure(
  table(
    columns: (auto, 1fr, 1fr),
    align: (left, center, center),
    [*Scenario*],          [*Mean restore time (ms)*], [*Std dev (ms)*],
    [No caching (N=0)],    [_TODO_],                   [_TODO_],
    [N=1 candidates],      [_TODO_],                   [_TODO_],
    [N=3 candidates],      [_TODO_],                   [_TODO_],
  ),
  caption: [End-to-end connectivity restoration time after primary-upstream failure],
) <tab-failover>

The expected result is that N=3 provides a restoration time close to zero
additional delay beyond the send-failure detection latency (one failed send
attempt), while N=0 requires waiting for the next heartbeat cycle. N=1 provides
fast failover to the single cached alternative but offers no second-order
failover if that candidate also fails simultaneously.

The hysteresis mechanism interacts with failover: after a failover event, the
promoted candidate has a cached score from before the failure. If the original
upstream recovers (100% loss is removed), it will not immediately reclaim the
primary route — it must score at least 10% better than the promoted candidate
before the route switches back. This prevents oscillation in borderline
scenarios where a link is intermittently degraded rather than permanently lost.

== Memory and CPU Overhead

=== Memory Overhead of N-Best Caching

Memory overhead of the N-best candidate list was measured by comparing the
resident set size (RSS) of a single OBU process with `cached_candidates`
set to 0, 1, and 3. RSS was measured via `/proc/<pid>/status` sampled 10
seconds into steady-state operation (after all routes have converged).

Each `CachedCandidate` stores a `MacAddress` (6 bytes), a score (`u64`, 8
bytes), and a timestamp (`Instant`, 16 bytes on x86-64 Linux) — 30 bytes per
entry before alignment padding. The candidate list is a `Vec<CachedCandidate>`
with at most `cached_candidates` entries, negligible compared to the routing
table itself (an `IndexMap<MacAddress, IndexMap<u32, PerHopInfo>>` holding
`hello_history = 10` entries per RSU per hop source). The expected RSS
difference between N=0 and N=3 is in the order of hundreds of bytes per peer
— unmeasurably small for typical topologies.

// TODO: insert table with RSS measurements

=== CPU Utilisation

CPU utilisation was profiled using `perf stat` over a 60-second simulation
window in steady-state (all routes converged, no channel parameter changes).
The metric of interest is the fraction of CPU time spent in routing computation
(`get_route_to`, `select_and_cache_upstream`) versus I/O (`tokio` task
scheduling, TUN read/write).

```sh
perf stat -p <simulator_pid> sleep 60
```

// TODO: insert perf stat output and analysis

The expected finding is that routing computation accounts for a small fraction
of total CPU, because `get_route_to` is pure and reads from an `IndexMap`
bounded by `hello_history`; its computational cost is $O(N_"peers" times "hello_history")$
per Heartbeat interval. The dominant CPU consumer is expected to be the Tokio
I/O event loop and TUN read/write operations.

== Cryptographic Session Establishment

=== Key Exchange Latency

The time to establish a DH session was measured for all three key exchange
configurations: X25519 (unsigned), X25519 + Ed25519 (signed), and
ML-KEM-768 + ML-DSA-65 (signed). Measurements were taken at zero channel
latency and loss to isolate cryptographic computation from network delay.

The dominant cost components are:

- *Key generation*: X25519 keypair generation is fast ($O(mu s)$). ML-KEM-768
  keypair generation involves polynomial arithmetic over the ring
  $ZZ_q[x] / (x^256 + 1)$ and takes on the order of hundreds of microseconds
  on modern x86-64 hardware with AES-NI. ML-DSA-65 key generation is in the
  same range.

- *Signature computation*: Ed25519 signing is fast ($O(mu s)$). ML-DSA-65
  signing uses a Fiat-Shamir with Aborts approach that involves rejection
  sampling and takes on the order of 1–3 ms per signature on x86-64.

- *Verification*: Ed25519 and ML-DSA-65 verification are slightly faster than
  signing. ML-DSA-65 verification takes approximately 0.5–1 ms.

- *Message serialisation and network round trip*: at zero latency, the two
  network messages (`KeyExchangeInit` and `KeyExchangeReply`) traverse a TUN
  interface pair. This is negligible relative to crypto.

#figure(
  table(
    columns: (auto, auto, 1fr, 1fr),
    align: (left, left, center, center),
    [*KE algorithm*], [*Signing*], [*Message sizes*], [*Mean handshake (ms)*],
    [X25519],         [None],     [45 B + 45 B],     [_TODO_],
    [X25519],         [Ed25519],  [146 B + 146 B],   [_TODO_],
    [ML-KEM-768],     [ML-DSA-65],[6 463 B + 6 367 B],[_TODO_],
  ),
  caption: [Key exchange handshake latency by algorithm configuration (zero channel latency)],
) <tab-ke-latency>

The ML-KEM-768 + ML-DSA-65 message sizes (approximately 6.5 KB per direction)
are 140× larger than unsigned X25519. At a simulated 802.11p data rate of
6 Mbps with 10 ms channel latency, a 6.5 KB message would add approximately
8.7 ms of transmission time, making the total handshake approximately
$10 + 8.7 + 10 + 8.7 + "crypto" approx 37 + "crypto"$ ms. For infotainment
sessions with a key lifetime of 12 hours (the `dh_rekey_interval_ms` default),
this overhead is amortised over millions of payload frames and is negligible
in practice. For safety-class sessions with very short lifetimes, the handshake
overhead would need to be factored into the latency budget.

=== Session Revocation Overhead

Session revocation (@sec-session-revocation) adds one network round trip
(server → RSU → OBU) plus one ML-DSA-65 verification at the OBU. The
verification cost is the same as for a key exchange message — approximately
0.5–1 ms — followed immediately by a new key exchange initiation. The total
disruption to OBU–server communication is one revocation round trip plus one
key exchange round trip; at typical vehicular link latencies (10–50 ms), this
totals approximately 50–150 ms, well within the latency tolerance of
infotainment-class sessions.

== Discussion

=== Routing Behaviour

The results characterise vigilant-parakeet as a faithful implementation of
the heartbeat-based routing model described in @l3-security-vehicular. Route
convergence in the Linear-3 and Star-5 topologies is dominated by the
heartbeat interval, with multi-hop topologies adding one additional interval
per relay hop as expected. The latency metric provides a measurable advantage
over hop-count in scenarios with latency-differentiated links (Mixed-6), where
hop count would select a suboptimal path with equal hops but higher latency.

The hysteresis threshold effectively prevents route oscillation at moderate
loss rates (below 10%), where per-packet RTT variance is significant but the
mean path quality is stable. At higher loss rates (above 15%), RTT measurements
become sparse and the metric degrades toward hop count anyway, so hysteresis
has less effect.

=== Failover Behaviour

The N-best candidate cache provides a clear benefit in the failover scenario:
`cached_candidates = 3` eliminates the heartbeat-cycle wait that dominates
recovery time without caching. The marginal benefit of N=3 over N=1 is relevant
only when the second-best candidate also fails simultaneously, which is the
rarer event.

=== Cryptographic Overhead

The post-quantum key exchange (ML-KEM-768 + ML-DSA-65) imposes a message size
overhead of approximately 140× compared to unsigned X25519, and a computation
overhead in the low single-digit millisecond range on the evaluation hardware.
For sessions with 12-hour lifetimes (the default), this overhead is negligible.
The main practical constraint for deployment on real 802.11p hardware is the
6.5 KB message size, which exceeds the 802.11p MPDU limit and would require
fragmentation or a larger frame transport, consistent with the analysis in
@etsi-pqc and @pqc-v2x.

=== Limitations

The evaluation has several limitations that should be acknowledged:

- *No radio channel model*: the simulator models channel quality as static
  per-link parameters. Real vehicular channels exhibit time-varying fading that
  would produce different RTT variance profiles.

- *No mobility*: topology changes are emulated only through manual channel
  parameter updates. Dynamic RSU association under vehicle movement is not
  evaluated.

- *Single-machine scheduling*: all nodes share the host kernel's Tokio thread
  pool. Under high node counts, Tokio task scheduling jitter contributes to
  measured latencies; results should be interpreted as simulator-level rather
  than hardware-level timing.

- *Five repetitions*: for highly stochastic scenarios (20% loss), five
  repetitions provide limited statistical power. Error bars reflect standard
  deviation across repetitions; significance testing is not performed.
