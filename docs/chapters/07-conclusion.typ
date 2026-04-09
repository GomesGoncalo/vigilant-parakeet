// ── Chapter 7 — Conclusion <conclusion> ──────────────────────────────────────

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
  communication via a configurable key exchange (classical X25519 or
  quantum-resistant ML-KEM-768), HKDF key derivation, and AEAD payload
  encryption (AES-256-GCM / AES-128-GCM / ChaCha20-Poly1305), with an
  optional digital signature layer (Ed25519 or ML-DSA-65) supporting TOFU
  and PKI deployment modes. ML-KEM-768 and ML-DSA-65 (NIST FIPS 203/204)
  provide post-quantum security against harvest-now-decrypt-later threats.

+ A *HeartbeatReply replay-detection mechanism* at RSUs (IPsec AH-style
  sliding receive window) and a *signed session-revocation protocol* with
  timestamp-and-nonce replay prevention, hardening the control plane against
  sequence-number forgery and revocation replay attacks.

+ An *in-process test infrastructure* (`Hub`, TUN shim) enabling
  deterministic, reproducible integration tests without root privileges.

+ A *browser-based visualisation dashboard* that renders live topology and
  traffic metrics from the simulator's HTTP API.

+ An *empirical evaluation* characterising route convergence time, metric
  sensitivity to packet loss, and the failover latency reduction provided
  by N-best caching (see @evaluation for results).

== Reflection on Design Decisions

Several design decisions proved particularly consequential during development.

*Separation of library and binary.* The decision to implement all node logic
in library crates (`obu_lib`, `rsu_lib`, `server_lib`) rather than in the
simulator directly enabled the test suite to run without privileges and without
network namespaces. Early prototypes embedded node logic directly in the
simulator binary; extracting it into libraries was the most impactful
refactoring, reducing the cost of adding each new integration test from "needs
sudo and namespace setup" to "just write a unit test." The lesson is that the
testability boundary should be designed into the architecture from the start,
not retrofitted.

*Userspace channel vs. `tc-netem`.* Using userspace `Channel` objects for link
emulation rather than kernel `tc-netem` rules was initially chosen for
simplicity, and turned out to provide an unexpected benefit: cooperation with
Tokio's mocked time. The mocked-time integration tests —
`integration_latency_measurement_mocked_time` and `integration_failover_send_error`
— could not be written against `tc-netem` without real wall-clock waits.
This validates the choice retrospectively.

*Configurable cipher suite.* Making the key exchange algorithm (X25519 vs.
ML-KEM-768), the signing algorithm (Ed25519 vs. ML-DSA-65), and the AEAD
cipher independently configurable at the YAML level, rather than hardcoding
a single algorithm pair, was more work upfront but essential for the
comparative evaluation of classical and post-quantum configurations. A single
`CryptoConfig` struct threading through all cryptographic operations made
this extensible: adding a new algorithm variant requires a new enum arm and
its implementation, without touching the handshake or encryption code paths.

*The hysteresis threshold.* The 30% hysteresis band was chosen empirically:
at lower thresholds (e.g. 10–15%), simulated links with moderate jitter
triggered route oscillation under fading; at thresholds above 40%, genuinely
better paths took too long to be adopted following a link-quality improvement.
The threshold is not configurable at the YAML level (an intentional
simplification), but the routing codebase is structured so that changing it
requires modifying a single constant. In a production system, the threshold
would ideally be derived from the observed jitter distribution of each link.

*TOFU as the default trust mode.* Implementing TOFU as the default (with PKI
mode available but opt-in) reflects the practical deployment reality that key
pre-distribution is operationally expensive. Most research use cases do not
require the security guarantees of mutual PKI authentication; they require
protection against passive eavesdroppers and accidental misconfigurations. TOFU
provides this at zero operational cost. The PKI mode is available for scenarios
where the full security property is required.

== Limitations

Several limitations remain despite the implemented extensions; the list
below highlights the most important caveats that affect experiment fidelity
and security conclusions.

- *Approximate radio model*: The simulator implements a Nakagami-m
  distance-based outage probability model (Chapter 9), which improves on static
  per-link parameters. However, it does not model frequency-selective fading,
  explicit Doppler spectra, antenna patterns, or MIMO spatial correlation present
  in real radios.

- *Mobility realism trade-offs*: OSM-driven trajectories with IDM (Chapter
  10) produce realistic longitudinal and lane-change dynamics, but they are
  not a substitute for trace-driven or SUMO-coupled microscopic scenarios for
  every use case; integrating SUMO remains a useful next step for richer
  traffic-control interaction.

- *Single-machine scaling*: The simulator runs all nodes on one host and
  shares kernel/network resources. At very large node counts scheduling and
  timing artifacts may appear; a distributed execution mode would reduce
  these host-sharing effects.

- *Control-plane authentication gap*: Heartbeat and HeartbeatReply messages
  remain unauthenticated; the RSU replay window detects replays but does not
  cryptographically prevent crafted fresh messages. HMAC or TESLA-style
  authentication remains a priority future improvement.

- *Operational PKI features absent*: While PKI-mode and signed handshakes
  are supported, there is no automatic certificate provisioning, short-lived
  pseudonymous certs, or online revocation (CRL/OCSP). Revocation requires
  manual server configuration updates in the current deployment model.

- *TOFU first-contact vulnerability*: Trust-on-first-use remains the
  default and carries the known risk that an active attacker at first contact
  can impersonate an endpoint; PKI mitigates this but requires provisioning.

These limitations should be borne in mind when interpreting experimental
results; where possible the evaluation emphasises relative comparisons (A vs
B) rather than absolute performance claims.
== Future Work

Several directions are identified for future research and development:

/ Heartbeat authentication: Apply HMAC @rfc2104 to unicast HeartbeatReply
  messages using the established DH session key, and investigate the TESLA
  @tesla delayed key disclosure protocol for broadcast Heartbeat messages.
  Quantifying the authentication latency penalty and its interaction with the
  routing hysteresis band would allow the routing-security trade-off to be
  evaluated empirically against the attacks described in @l3-security-vehicular.

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
