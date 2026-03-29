// ── Chapter 6 — Conclusion <conclusion> ──────────────────────────────────────

= Conclusion <conclusion>

== Summary

This thesis presented vigilant-parakeet, a Rust-based simulator and
visualiser for vehicular network routing protocols. The system runs real
node logic inside Linux network namespaces, providing a faithful emulation
environment that does not require physical hardware or external network
simulators.

The key contributions were:

+ A *modular Cargo workspace* architecture that cleanly separates shared
  protocol logic (`node_lib`, `common`) from node-type implementations
  (`obu_lib`, `rsu_lib`) and the orchestration layer (`simulator`).

+ A *heartbeat-based routing protocol* with a composite latency/hop-count
  metric, hysteresis-protected route caching, and N-best upstream candidate
  caching for fast failover.

+ An *in-process test infrastructure* (`Hub`, TUN shim) enabling
  deterministic, reproducible integration tests without root privileges.

+ A *browser-based visualisation dashboard* that renders live topology and
  traffic metrics from the simulator's HTTP API.

+ An *empirical evaluation* characterising route convergence time, metric
  sensitivity to packet loss, and the failover latency reduction provided
  by N-best caching.

// TODO: fill in key numerical results from Chapter 5

== Limitations

Several limitations should be acknowledged:

- *No radio channel model*: The simulator models channel quality as static
  per-link latency and loss parameters. Real vehicular channels exhibit
  time-varying fading and Doppler effects that are not captured.

- *No mobility model*: Node positions are fixed for the duration of a
  simulation run. Dynamic topology changes (vehicles joining and leaving
  radio range) are emulated only through manual channel parameter updates
  via the API.

- *Single-machine simulation*: All nodes share the host kernel and
  CPU. At high node counts the shared thread pool may introduce scheduling
  artefacts that would not appear in a distributed deployment.

- *No authentication*: The current implementation does not implement the
  cryptographic authentication mechanisms proposed in @l3-security-vehicular.
  The `crypto/` module provides a skeleton for future work.

== Future Work

Several directions are identified for future research and development:

/ Dynamic mobility: Integrate a SUMO @sumo mobility trace reader to drive
  link up/down events and channel quality changes from realistic vehicle
  mobility patterns.

/ Radio channel model: Replace the static `tc netem` model with a
  time-varying channel model (e.g., Nakagami-m fading) to better
  represent real vehicular propagation.

/ Authentication and integrity: Implement the L3 security mechanisms from
  @l3-security-vehicular, including HMAC-based heartbeat authentication
  and replay-attack detection, to evaluate their overhead and effectiveness.

/ Distributed simulation: Extend the simulator to distribute nodes across
  multiple physical machines connected by an overlay network, removing the
  single-machine bottleneck.

/ Formal verification: Apply model checking (e.g., TLA+) to the routing
  state machine to verify absence of forwarding loops and liveness of
  route convergence under the current protocol specification.
