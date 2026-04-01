// ── Chapter 3 — Architecture <architecture> ──────────────────────────────
#import "@preview/fletcher:0.5.7" as fletcher: diagram, node, edge
#import "@preview/chronos:0.2.1"

= Architecture <architecture>

This chapter describes the high-level design of vigilant-parakeet: the crate
decomposition, the three-tier network model, and the key design decisions.

== Workspace Overview

vigilant-parakeet is organised as a Rust Cargo workspace with nine crates
(@fig-workspace-crates). The central principle is strict separation of concerns:
shared protocol code lives in `node_lib` and `common`; node-type behaviour is
encapsulated in `obu_lib`, `rsu_lib`, and `server_lib`; orchestration and
analysis tooling sit in `simulator` and `scripts_tools`.

#figure(
  diagram(
    node-stroke: 0.5pt,
    spacing: (18mm, 10mm),
    node((0,0), [`simulator`]),
    node((0,2), [`node`]),
    node((0,4), [`visualization`]),
    node((0,5), [`scripts_tools`]),
    node((1,0), [`node_factory`]),
    node((1,4), [Simulator\ HTTP API]),
    node((2,0), [`obu_lib`]),
    node((2,1), [`rsu_lib`]),
    node((2,2), [`server_lib`]),
    node((3,1), [`node_lib`]),
    node((4,1), [`common`]),
    edge((0,0), (1,0), "->"),
    edge((1,0), (2,0), "->"),
    edge((1,0), (2,1), "->"),
    edge((1,0), (2,2), "->"),
    edge((0,2), (2,0), "->"),
    edge((0,2), (2,1), "->"),
    edge((0,2), (2,2), "->"),
    edge((2,0), (3,1), "->"),
    edge((2,1), (3,1), "->"),
    edge((2,2), (3,1), "->"),
    edge((3,1), (4,1), "->"),
    edge((0,4), (1,4), "->"),
  ),
  caption: [Cargo workspace dependency graph],
) <fig-workspace-crates>

== Three-Tier Network Model

The system implements a three-tier network that separates concerns between
wireless VANET communication, infrastructure forwarding, and application-layer
connectivity (@fig-three-tier).

#figure(
  diagram(
    node-stroke: 0.5pt,
    node-inset: 10pt,
    spacing: (0mm, 8mm),
    node((0,0),
      align(center)[*Tier 1 — VANET medium*  (10.x.x.x)\
      OBU $arrow.r$ RSU\
      Heartbeat, HeartbeatReply, Data, KeyExchange],
      width: 100mm,
    ),
    edge((0,0), (0,1), "->", [UDP cloud protocol], label-side: center),
    node((0,1),
      align(center)[*Tier 2 — Cloud / infrastructure*  (172.x.x.x)\
      RSU $arrow.r$ Server\
      UpstreamForward, DownstreamForward,\
      KeyExchangeForward, KeyExchangeResponse],
      width: 100mm,
    ),
    edge((0,1), (0,2), "->", [decapsulate + decrypt], label-side: center),
    node((0,2),
      align(center)[*Tier 3 — Virtual TAP*  (overlay L2)\
      OBU virtual TAP $arrow.l$ Server virtual TAP\
      Decrypted IPv4/IPv6 payload],
      width: 100mm,
    ),
  ),
  caption: [Three-tier network architecture],
) <fig-three-tier>

- *Tier 1 (VANET)* is the wireless medium. OBUs and RSUs exchange routing
  control messages (Heartbeat, HeartbeatReply) and carry encrypted payload
  frames in the `Data` message type.

- *Tier 2 (cloud)* is the infrastructure link between RSUs and the server,
  implemented over UDP. RSUs are transparent relays: they wrap received VANET
  frames in `UpstreamForward` messages and send them to the server without
  inspecting or decrypting the payload.

- *Tier 3 (virtual TAP)* is an overlay L2 network shared between OBUs and
  the server. After decryption, the server injects plaintext frames into its
  virtual TAP device; the OBU reads decrypted frames from its own virtual TAP.

== Crate Responsibilities

=== `common` — OS Abstractions

Provides platform-level building blocks:
- *`Tun` trait* with a `TokioTun` test shim for non-privileged integration testing.
- *`Device`* — wraps a network interface for send/receive.
- *`NetworkInterface`*, *`ChannelParameters`* — interface configuration helpers.
- *`stats`* — optional feature-gated atomic counters.

=== `node_lib` — Protocol Building Blocks

Cross-cutting protocol logic shared by all node types:
- *`messages/`* — wire encoders/decoders: `Heartbeat`, `HeartbeatReply`, `Data`, `KeyExchangeInit`, `KeyExchangeReply`, `PacketType`, `Message`.
- *`crypto/`* — full configurable cipher suite: X25519 DH, HKDF, AES-256-GCM / AES-128-GCM / ChaCha20-Poly1305.
- *`data/`* — data-path helpers split by node type (`data::obu`, `data::rsu`).
- *`control/`* — shared routing utilities: `route`, `routing_utils`, `client_cache`, `ReplyType`.
- *`args/`* — `NodeType` enum (`Rsu | Obu | Server`) used by the node binary and simulator.
- *`buffer_pool`* — reusable fixed-size buffers to reduce hot-path allocation.
- *`Node` trait* — `trait Node: Send + Sync` implemented by `Obu`, `Rsu`, and `Server` for uniform handling in the simulator.
- *`test_helpers/`* — `Hub` and `mk_shim_pair()` for non-privileged integration testing.

=== `obu_lib` — OBU Node

Implements the full OBU control plane:
- Heartbeat reception, routing table management (~1117-line `Routing`), N-best upstream caching and failover.
- DH key store (`DhKeyStore`): pending → established key lifecycle per server, with retry and expiry.
- TAP session: reads frames from the virtual TAP, encrypts them, and sends them upstream.
- Two network interfaces: `vanet` TAP (tier 1) and `virtual` TAP (tier 3).

=== `rsu_lib` — RSU Node

Implements the RSU control plane:
- Periodic Heartbeat emission and HeartbeatReply processing.
- Client cache (`ClientCache`): tracks OBU VANET MAC → virtual MAC associations.
- *No TUN/TAP device*: RSUs only have a `vanet` interface and a `cloud` UDP socket. They never decrypt or inspect OBU payload.
- Forwards VANET data opaquely to the server as `UpstreamForward` UDP messages.

=== `server_lib` — Server Node

The end-to-end trust anchor:
- Listens on a UDP `cloud` socket (tier 2) for RSU-forwarded messages.
- Owns a `virtual` TAP device (tier 3) for decapsulated application traffic.
- Manages per-OBU DH-derived keys; decrypts `UpstreamForward` payloads.
- Routes downstream traffic back to the correct RSU using an `obu_routes` table (`virtual TAP MAC → (VANET MAC, RSU UDP addr)`).
- RSU registration: `RegistrationMessage` keeps `RSU MAC → [OBU MACs]` up to date.
- The binary cloud protocol uses magic prefix `[0xAB, 0xCD]` and a 1-byte type discriminant (see @security for wire formats).

=== `node` — Single-Node Binary

Three CLI subcommands: `node obu`, `node rsu`, `node server`. Each delegates directly to the corresponding library's `create()` function.

=== `simulator` — Multi-Node Orchestration

- Reads a YAML configuration describing nodes and topology.
- Creates one Linux network namespace per node.
- Builds nodes via `node_factory::create_node_from_settings()`.
- Applies per-link latency, loss, and jitter via in-process `Channel` objects.
- Exposes HTTP API (port 3030, feature: `webview`) and TUI dashboard (feature: `tui`).
- TUI has seven tabs: Metrics, Logs, Nodes, Topology, Channels, Upstreams, Registry.

=== `visualization` — Browser Dashboard

Yew/WASM application that polls the simulator's HTTP API and renders live topology and traffic charts.

=== `scripts_tools` — Experiment Analysis CLI

A standalone binary for processing experiment data:
`parse-band`, `build-summary`, `merge-latency`, `ns-addrs`, `generate-pairs`, `validate-configs`, `autofix-configs`.

== Key Design Decisions

=== RSU Opacity

RSUs deliberately never hold session keys. All OBU payload encryption is
end-to-end between the OBU and the server. Compromising an RSU exposes only
routing metadata, not payload data.

=== Pure Route Selection

`get_route_to(Some(mac))` is a pure, read-only function. `select_and_cache_upstream()` is the sole write path for updating the cached upstream. This prevents hidden mutations under `RwLock` read guards.

=== Feature-Gated Instrumentation

The `stats` feature compiles in atomic counters (`loop_detected_count`, etc.) at zero cost when disabled. The `webview` and `tui` features are similarly additive.

=== Async-First, No Threads per Node

All node I/O runs as Tokio async tasks on a shared thread pool, enabling dense simulations (dozens of nodes) on commodity hardware without per-node OS threads.

=== Test Infrastructure Without Root

The `test_helpers::Hub` and `TokioTun` shim allow full integration tests of multi-hop routing, encryption, and failover without creating kernel interfaces or requiring elevated privileges.

