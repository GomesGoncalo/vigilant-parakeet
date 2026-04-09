// ── Chapter 11 — Visualization enhancements <viz-enhancements> ──

= Visualization enhancements <viz-enhancements>

This chapter documents the visualization improvements added to the dashboard
and how they support experimental reproducibility, real-time debugging and
presentation-quality demonstrations.

== Smooth marker animation

To visualise vehicle motion without stutter, the map tab uses requestAnimationFrame
(rAF) driven interpolation. The dashboard maintains a short history of each
node's last known and current positions (lat/lon + timestamp). On each rAF
tick a time-aligned interpolated position is computed and `marker.setLatLng`
is called, producing smooth continuous motion independent of the `/node_info`
polling interval.

== Directional routing arrows and edge styling

Edges representing routing upstream flow are drawn as lightweight canvas polylines
with small arrowheads indicating direction. Edge colour encodes a composite
health metric (latency+loss) and line thickness reflects observed throughput.
Arrows are drawn only for active upstream flows to reduce visual clutter.

== Performance and polling optimisations

Large simulations require careful rendering choices. The dashboard:

+ Bypasses Yew/WASM for high-frequency position updates using a JS-native
  fetch path that updates Leaflet layers imperatively, reducing WASM
  round-trips and improving latency.
+ Caches icon bitmaps and groups markers by type to reduce DOM overhead.
+ Supports configurable polling rates and a delta-update mode where only
  changed node fields are applied.

== Observability features

Each marker popup includes:

+ Recent RSSI samples (smoothed window), with sparkline mini-graph.
+ N-best candidate list with timestamps and the score components (min, mean,
  normalised RSSI) used for selection.
+ Recent Key Exchange attempts and their outcome (success/failure, latency).

These features make the dashboard a core tool for diagnosing routing flaps,
KE failures, and for producing qualitative figures for presentations.

== Usage notes

- For screenshots and demonstrations the dashboard supports a pause-and-step
  mode where the latest polled state is frozen and navigable.
- The map auto-centres on the visible vehicular nodes when the Map tab is
  selected, ensuring consistent framing across repeated runs.
