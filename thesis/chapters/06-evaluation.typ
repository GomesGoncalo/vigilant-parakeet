// ── Chapter 5 — Evaluation <evaluation> ──────────────────────────────────────

= Evaluation <evaluation>

== Experimental Setup

All experiments were conducted on a single host machine running Linux 6.x
with the following specification:

// TODO: fill in actual hardware details
- CPU: _[processor model]_
- RAM: _[amount]_ GB
- OS: Ubuntu 22.04 LTS (kernel 6.x)

The simulator was built in release mode with the `webview` and `stats` feature
flags enabled:

```sh
cargo build -p simulator --release --features "webview,stats"
```

Each experiment was repeated five times; results report the mean and standard
deviation unless otherwise stated.

== Topology Configurations

Three reference topologies were used:

/ Linear-3: Three nodes in a chain: RSU — OBU₁ — OBU₂. Tests basic
  two-hop routing and reply forwarding.

/ Star-5: One RSU and four OBUs, all directly connected to the RSU. Tests
  single-hop performance at higher node density.

/ Mixed-6: Two RSUs and four OBUs arranged in a partial mesh with
  asymmetric links. Tests route selection under competing RSU signals and
  asymmetric latency.

Topology YAML files for all three configurations are provided in the
`examples/` directory of the repository.

== Route Convergence

Route convergence time was measured as the interval between simulator
start and the first moment all OBUs had a valid cached upstream entry, as
reported by the `/metrics` endpoint.

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

// TODO: insert actual measurements and discussion

== Route Quality Under Packet Loss

To evaluate the metric's sensitivity to channel degradation, the loss
parameter on the RSU–OBU₁ link in the Linear-3 topology was varied from
0% to 20% in 2% steps using the runtime channel API:

```sh
curl -X POST -H "Content-Type: application/json" \
  -d '{"latency":"10","loss":"0.05"}' \
  http://localhost:3030/channel/rsu/obu1/
```

// TODO: insert figure showing selected route metric score vs. loss rate

== N-Best Failover Latency

The benefit of the N-best candidate cache was measured by inducing a
primary-upstream failure (by setting 100% loss on the active link) and
timing the restoration of end-to-end connectivity.

#figure(
  table(
    columns: (auto, 1fr, 1fr),
    align: (left, center, center),
    [*Scenario*],          [*Mean restore time (ms)*], [*Std dev (ms)*],
    [No caching],          [_TODO_],                   [_TODO_],
    [N=1 candidates],      [_TODO_],                   [_TODO_],
    [N=3 candidates],      [_TODO_],                   [_TODO_],
  ),
  caption: [Failover restoration time with and without N-best caching],
) <tab-failover>

// TODO: fill in measurements and discuss hysteresis effect

== Memory and CPU Overhead

Memory overhead of the N-best candidate list was measured by comparing the
resident set size (RSS) of a single OBU process with `cached_candidates`
set to 0, 1, and 3.

CPU utilisation was profiled using `perf stat` over a 60-second simulation
window.

// TODO: insert table / figure and discussion

== Discussion

// TODO: summarise findings, compare to prior work (@l3-security-vehicular),
// acknowledge limitations (single-machine simulation, no real radio channel,
// no mobility model)
