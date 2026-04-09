// ── Chapter 8 — Routing rework and algorithmic changes <routing-rework> ──

= Routing rework and algorithmic changes <routing-rework>

This chapter documents the changes made to the heartbeat-based routing
algorithm during the development of the system. The goal of the rework was to
improve upstream stability under time-varying channels and mobility, to make
selection policies more expressive (RSSI-aware policies), and to make the
behaviour easier to reason about and test.

== Motivation

The original composite metric (min + mean latency) provides good discrimination
between candidates but is sensitive to single-sample minima under high-variance
channels (e.g., small-scale fading) and to transient packet loss. In practice
this manifested as RSU flapping when vehicles moved through fading dips or when
probe timing coincided with brief congestion. The rework addresses these
issues by adding a small set of orthogonal mechanisms: configurable scoring
modes, RSSI-aware selection, and stronger hysteresis.

== Scoring modes

Two scoring modes are supported:

+ `min+mean` (default): `s_m = mu_m^min + overline(mu)_m`. Rewards low minima while
  penalising variance via the mean.

+ `avg-only`: `s_m = overline(mu)_m`. Useful when minima are unrepresentative
  due to sampling noise or measurement artifacts.

Choosing between the two is a configuration-time policy decision. The code
exposes this via the node YAML under the OBU's routing section.

== RSSI-aware selection

When enabled, OBUs collect per-RSU RSSI samples and smooth them with a short
moving-average window. Each candidate obtains a normalised RSSI score r_m in
[0,1] (higher values indicate stronger received power). A combined score is
then computed as

  `combined_m = w_latency * normalise(s_m) + w_rssi * (1 - r_m)`

where normalise() maps observed latency scores into [0,1] (lower is better),
and w_latency + w_rssi = 1. The subtraction (1 - r_m) ensures lower combined
values remain better. The weights w_latency and w_rssi are configurable, which
permits tuning for environments where physical signal strength better predicts
sustained capacity than instantaneous latency.

RSSI-aware selection reduces unnecessary handovers when the signal strength
indicates a stable link even if transient latency samples occasionally appear
better for other RSUs.

== Hysteresis and failover

The hysteresis band was increased (default 30%). A candidate is adopted only
when its (possibly combined) score is X% lower than the cached upstream's score
(or when it offers fewer hops). This change dramatically reduces oscillation
in mobility-plus-fading scenarios. The previously described N-best cache and
O(1) failover promotion remain in place: the top N candidates are selected on
primary-route updates and promoted in constant time on failure.

== Test coverage and observability

Unit and integration tests exercise the new modes: synthetic channels with
controlled jitter and fading verify that `avg-only` resists minima-driven
flapping, RSSI-weighted selection follows expected policies, and the hysteresis
parameter prevents pathological oscillation. The dashboard and `/node_info`
endpoints expose the scoring components (raw latencies, smoothed RSSI samples,
normalised scores) to make post-hoc analysis straightforward.

== Practical guidance

For experimentation:

- Use `avg-only` in high-variance fading scenarios or when probe timing is
  loosely synchronised with channel coherence times.
- Use RSSI weighting in sparse RSU deployments where signal strength is a
  reliable predictor of link stability.
- Start with a conservative hysteresis (30%) and reduce if the environment is
  extremely stable and fast handover is desirable.
