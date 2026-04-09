// ── Chapter 1 — Introduction ──────────────────────────────────────────────────

= Introduction <introduction>

== Motivation

Vehicular networks — also known as VANETs (Vehicular Ad-hoc Networks) — are a
class of mobile ad-hoc network in which vehicles and roadside infrastructure
communicate to improve road safety, traffic efficiency, and passenger
experience. The combination of high node mobility, intermittent connectivity,
and safety-critical applications makes routing a particularly demanding problem
in this domain.

Existing simulation tools either require costly proprietary hardware, operate
at the application layer only, or fail to model the per-link latency and packet
loss that characterise real vehicular channels. This thesis addresses that gap
by presenting *vigilant-parakeet*: a Linux-native, open-source simulator that
runs real Layer-3 node logic inside isolated network namespaces, allowing
researchers to study routing protocols under controlled but realistic network
conditions.

The project was originally motivated by the vehicular-network security work of
@l3-security-vehicular, whose routing model forms the basis of the protocol
implemented here.

== Research Questions

This thesis investigates the following questions:

+ Can a single-machine Linux simulator faithfully reproduce the routing
  dynamics of a multi-hop vehicular network without specialised hardware?

+ How does the choice of routing metric (latency vs. hop count) affect
  convergence time and route stability under varying loss conditions?

+ What is the overhead introduced by N-best candidate caching on memory and
  CPU, and how much does it reduce route-restoration latency after a next-hop
  failure?

+ What is the practical overhead of post-quantum key exchange (ML-KEM-768
  + ML-DSA-65) in terms of handshake latency and message size, and is it
  compatible with the latency budgets of vehicular V2I sessions?

== Contributions

The primary contributions of this work are:

- *A modular Rust simulator* (`vigilant-parakeet`) built as a Cargo workspace,
  separating shared protocol logic (`node_lib`) from node-type implementations
  (`obu_lib`, `rsu_lib`) and the orchestration layer (`simulator`).

- *A heartbeat-based routing protocol* with latency-aware metric computation,
  N-best upstream candidate caching, and fast failover.

- *An end-to-end security architecture* providing encrypted OBU–server
  communication via a configurable key exchange (classical X25519 or
  quantum-resistant ML-KEM-768, NIST FIPS 203), HKDF-derived session keys,
  and AEAD payload encryption (AES-256-GCM, AES-128-GCM, ChaCha20-Poly1305).
  An optional digital signature layer (Ed25519 or ML-DSA-65, NIST FIPS 204)
  protects the key exchange itself, supporting both Trust-on-First-Use (TOFU)
  and pre-registered PKI deployment modes. The post-quantum combination of
  ML-KEM-768 and ML-DSA-65 addresses harvest-now-decrypt-later threats against
  long-lived vehicular infrastructure.

- *A HeartbeatReply replay-detection mechanism* (`rsu_lib::ReplayWindow`) at
  RSUs — a per-sender sliding 64-bit bitmask window following the IPsec AH
  design — preventing stale routing state from being injected via replayed
  control messages. A supplementary window-poisoning defence prevents forged
  large sequence numbers from rendering the window permanently closed.

- *A signed session-revocation protocol* allowing the server to forcibly
  terminate an OBU's established DH session and trigger immediate re-keying.
  Revocation messages carry a timestamp and a fresh random nonce, with the OBU
  maintaining a time-bounded nonce cache to prevent replay over the revocation
  validity window.

- *A `keygen` utility* for generating Ed25519 and ML-DSA-65 signing keypairs
  at provisioning time, enabling stable, restartable node identities for PKI
  mode deployments.

- *An in-process test hub* (`node_lib::test_helpers::hub::Hub`) enabling
  deterministic, reproducible integration testing without root privileges or
  physical network devices.

- *A browser-based visualisation dashboard* consuming a real-time HTTP metrics
  API exposed by the simulator.

- *Time-varying radio model and mobility*: the simulator supports a
  Nakagami-m small-scale fading model (configurable m and sampling granularity)
  and an OpenStreetMap-driven mobility backend with an IDM car-following model
  for realistic vehicle trajectories and reproducible experiments.

- *RSSI-aware and reworked routing*: the heartbeat-based routing protocol was
  extended with configurable scoring modes (min+mean, avg-only), RSSI-aware
  selection, and stronger hysteresis to mitigate RSU flapping under fading and
  mobility (details in Chapters 4 and 8).

- *Key-exchange robustness improvements*: downstream client caching, reply
  forwarding via a ClientCache, and prompt DH retry behaviours improve session
  establishment reliability under churn and high-loss conditions; the core
  security architecture (X25519 / ML-KEM-768, HKDF, AEAD) remains central to
  the design.

- *An empirical evaluation* of routing behaviour across a range of topologies,
  latency profiles, and packet-loss rates.

== Thesis Structure

The remainder of this thesis is organised as follows.

- @background reviews vehicular networking concepts, relevant routing
  protocols, post-quantum cryptographic foundations, and simulation approaches.

- @related-work surveys related VANET simulation platforms, routing protocol
  implementations, and security frameworks, positioning vigilant-parakeet
  within the existing literature.

- @architecture describes the high-level design of vigilant-parakeet and the
  rationale behind its crate decomposition.

- @implementation details the implementation of the routing protocol, the
  simulator, the test infrastructure, and the visualisation layer.

- @security presents the security architecture: threat model, DH key exchange
  (X25519 and ML-KEM-768), HKDF key derivation, configurable AEAD cipher
  suite, DH key store lifecycle, the Ed25519/ML-DSA-65 authentication layer
  with its TOFU and PKI trust models, HeartbeatReply replay detection, and
  the signed session-revocation protocol.

- @evaluation presents the experimental setup and results.

- @conclusion summarises the findings, reflects on limitations, and proposes
  directions for future work.
