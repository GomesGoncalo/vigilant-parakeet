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
via X25519 Diffie-Hellman key exchange, HKDF-derived session keys, and AEAD
payload encryption; an optional Ed25519 authentication layer defends the key
exchange against man-in-the-middle attacks in both Trust-on-First-Use and
pre-registered PKI deployment modes. The system exposes an HTTP control API
and a browser-based visualisation dashboard for interactive experimentation.

Key contributions include: a modular Rust workspace architecture separating
shared protocol logic from node-specific implementations; a configurable
cipher suite (AES-256-GCM, AES-128-GCM, ChaCha20-Poly1305) with transparent
relay opacity ensuring intermediate RSUs cannot access session keys; an
in-process programmable hub for deterministic, reproducible testing without
root privileges; and an evaluation of routing behaviour under varying latency
and packet-loss conditions.

Results demonstrate that the simulator faithfully reproduces the routing
dynamics described in prior vehicular-network security literature, and that
the N-best failover mechanism reduces route-restoration latency by up to
_X_ ms compared to a naïve single-upstream approach.
