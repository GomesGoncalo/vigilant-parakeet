// ── Abstract ──────────────────────────────────────────────────────────────────

Vehicular networks present unique challenges for routing protocol design: nodes
are highly mobile, connectivity is intermittent, and security requirements are
stringent. This thesis presents the design, implementation, and evaluation of
*vigilant-parakeet*, a Rust-based simulator and visualiser for experimenting
with Layer-3 routing protocols in vehicular networks composed of On-Board Units
(OBUs) and Road-Side Units (RSUs).

The simulator leverages Linux network namespaces to provide realistic, isolated
per-node network stacks without requiring physical hardware. A heartbeat-driven
routing protocol based on latency and hop-count metrics is implemented, with
support for N-best candidate caching and rapid upstream failover. A full
security architecture provides end-to-end encrypted OBU–server communication
via a configurable key exchange (classical X25519 or quantum-resistant
ML-KEM-768), HKDF-derived session keys, and AEAD payload encryption
(AES-256-GCM, AES-128-GCM, or ChaCha20-Poly1305); an optional digital
signature layer (Ed25519 or ML-DSA-65) defends the key exchange against
man-in-the-middle attacks in both Trust-on-First-Use and pre-registered PKI
deployment modes. A signed session-revocation mechanism allows the server to
forcibly terminate and re-key any OBU session, protected against replay by
a timestamp-and-nonce scheme. The system exposes an HTTP control API
and a browser-based visualisation dashboard for interactive experimentation.

Key contributions include: a modular Rust workspace architecture separating
shared protocol logic from node-specific implementations; a configurable
cipher suite with transparent relay opacity ensuring intermediate RSUs cannot
access session keys; post-quantum security via ML-KEM-768 key encapsulation
and ML-DSA-65 digital signatures (NIST FIPS 203/204), addressing harvest-now-
decrypt-later threats; a HeartbeatReply replay-detection window following the
IPsec AH design; an in-process programmable hub for deterministic, reproducible
testing without root privileges; and an evaluation of routing behaviour under
varying latency and packet-loss conditions.
