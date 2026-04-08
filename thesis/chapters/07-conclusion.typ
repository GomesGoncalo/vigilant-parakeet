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

*The hysteresis threshold.* The 10% hysteresis band was chosen empirically:
at 5%, simulated links with moderate jitter triggered route oscillation during
evaluation; at 20%, genuinely better paths took too long to be adopted. The
threshold is not configurable at the YAML level (an intentional simplification),
but the routing codebase is structured so that changing it requires modifying
a single constant. In a production system, the threshold would ideally be
derived from the observed jitter distribution of each link.

*TOFU as the default trust mode.* Implementing TOFU as the default (with PKI
mode available but opt-in) reflects the practical deployment reality that key
pre-distribution is operationally expensive. Most research use cases do not
require the security guarantees of mutual PKI authentication; they require
protection against passive eavesdroppers and accidental misconfigurations. TOFU
provides this at zero operational cost. The PKI mode is available for scenarios
where the full security property is required.

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
  messages carry no HMAC or signature. While HeartbeatReply replay is
  detected at the RSU via a sliding receive window (@sec-routing-protocol),
  an adversary can still *inject* fresh-looking control messages or
  manipulate routing tables through crafted (rather than replayed) messages
  without cryptographic detection, as described in @l3-security-vehicular.


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
