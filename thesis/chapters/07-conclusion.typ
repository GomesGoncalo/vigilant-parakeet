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

+ A *security architecture* providing end-to-end encrypted OBU–server
  communication via X25519 Diffie-Hellman key exchange, HKDF key derivation,
  and AEAD payload encryption (AES-256-GCM / AES-128-GCM / ChaCha20-Poly1305),
  with an optional Ed25519 authentication layer supporting both TOFU and
  PKI deployment modes.

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

- *TOFU first-contact vulnerability*: In TOFU authentication mode, an active
  adversary present during the very first key exchange can impersonate either
  endpoint by substituting a different Ed25519 signing key. PKI mode closes
  this gap but requires out-of-band key provisioning.

- *No certificate revocation*: The PKI allowlist is a static YAML map.
  A compromised OBU identity cannot be invalidated without redeploying the
  server configuration. There is no support for short-lived certificates,
  certificate revocation lists (CRLs), or online revocation checking.

- *Control-plane messages unauthenticated*: Heartbeat and HeartbeatReply
  messages carry no HMAC or signature. An adversary on any VANET link can
  inject or replay control messages to manipulate routing tables without
  cryptographic detection, as described in @l3-security-vehicular.


== Future Work

Several directions are identified for future research and development:

/ Dynamic mobility: Integrate a SUMO @sumo mobility trace reader to drive
  link up/down events and channel quality changes from realistic vehicle
  mobility patterns. This would allow evaluation of route convergence under
  the intermittent connectivity that characterises real vehicular deployments.

/ Radio channel model: Replace the static userspace latency/loss model with a
  time-varying channel model (e.g., Nakagami-m fading) to better
  represent real vehicular propagation, including Doppler spread and
  rapid link quality fluctuations at vehicular speeds.

/ Heartbeat authentication: Apply HMAC @rfc2104 to unicast HeartbeatReply
  messages using the established DH session key, and investigate the TESLA
  @tesla delayed key disclosure protocol for broadcast Heartbeat messages.
  Quantifying the authentication latency penalty and its interaction with the
  routing hysteresis band would allow the routing-security trade-off to be
  evaluated empirically against the attacks described in @l3-security-vehicular.

/ Replay protection: Add sequence-number-based replay detection to the
  control plane. HeartbeatReply messages are currently accepted regardless of
  recency; a sliding receive window (as in IPsec AH) would prevent replayed
  messages from being used to maintain stale routing entries.

/ Certificate-based PKI: Replace the static YAML allowlist with a full X.509
  or ETSI ITS @ieee-1609-2 certificate infrastructure, enabling dynamic OBU
  enrolment, short-lived pseudonymous certificates, and revocation via CRLs.
  This would align the data-plane authentication model with deployed V2X
  security standards while retaining the lightweight DH handshake for session
  key establishment.

/ Sybil detection: Integrate a Sybil detection module @sybil that correlates
  routing-table observations from multiple RSUs to identify nodes advertising
  more simultaneous VANET identities than their radio capability permits.
  The simulator's programmable Hub makes it straightforward to inject
  synthetic Sybil traffic into integration tests.

/ Distributed simulation: Extend the simulator to distribute nodes across
  multiple physical machines connected by an overlay network, removing the
  single-machine bottleneck and enabling larger-scale experiments.

/ Formal verification: Apply model checking (e.g., TLA+) to both the routing
  state machine and the DH authentication protocol to verify absence of
  forwarding loops, liveness of route convergence, and the absence of
  authentication bypass under the threat model of @sec-trust-models.
