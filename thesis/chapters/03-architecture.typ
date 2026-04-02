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

Three CLI subcommands: `node obu`, `node rsu`, `node server`. Each delegates
directly to the corresponding library's `create()` function. This thin binary
layer means the same library code used in integration tests is also the
production entrypoint — there is no separate "simulation mode."

=== `simulator` — Multi-Node Orchestration

The simulator is the central orchestration layer. Its responsibilities are:

+ *Configuration loading*: reads a YAML file describing node types, per-node
  config paths, and a topology matrix specifying which nodes are adjacent and
  with what baseline channel parameters.

+ *Namespace provisioning*: creates one Linux network namespace per node using
  `netns_rs::NetNs::new("sim_ns_<name>")`. Each namespace has an independent
  network stack; nodes cannot communicate except through the TUN/TAP interfaces
  the simulator creates.

+ *Interface and node creation*: delegates to `node_factory`, which creates the
  correct set of virtual interfaces inside the namespace context and instantiates
  the appropriate library (`obu_lib`, `rsu_lib`, `server_lib`).

+ *Channel management*: each directed link is represented by a `Channel`
  object that applies configurable latency, loss, and jitter entirely in
  userspace. Channel parameters can be updated at runtime without restarting
  any node.

+ *Observability*: exposes an HTTP metrics API (feature: `webview`, port 3030)
  and a terminal TUI dashboard (feature: `tui`).

The simulator architecture is described in full in @implementation.

=== `visualization` — Browser Dashboard

A Yew/WASM application compiled to WebAssembly and served from a static HTTP
server. It polls the simulator's `/node_info` endpoint on a configurable
interval and renders:

- A live topology graph showing nodes and active links, with per-link
  channel parameters.
- Per-node traffic counters updated in real time.
- Upstream routing state for each OBU, showing the currently selected relay
  path toward each RSU.

The visualisation is purely read-only and stateless: all state lives in the
simulator; the browser is a rendering frontend with no persistent storage.

=== `scripts_tools` — Experiment Analysis CLI

A standalone binary for processing experiment data collected from simulator
runs:

/ `parse-band`: parses bandwidth measurement logs into structured records.
/ `build-summary`: aggregates per-node per-experiment metrics into a summary CSV.
/ `merge-latency`: combines latency measurements from multiple experiment runs.
/ `ns-addrs`: extracts namespace IP/MAC address assignments for correlation.
/ `generate-pairs`: generates node-pair configuration matrices for sweep experiments.
/ `validate-configs`: checks YAML configuration files for structural errors
  before running the simulator.
/ `autofix-configs`: applies a set of automated corrections to common
  configuration mistakes.

== Simulator Architecture in Depth <sec-simulator-arch>

=== Network Namespace Isolation

Linux network namespaces provide the isolation primitive underlying the entire
simulation. Each namespace is a separate instantiation of the kernel's network
stack: it has its own loopback interface, its own routing table, its own
`iptables` rule set, and its own set of sockets. Processes inside a namespace
see only the interfaces assigned to that namespace.

The simulator creates one namespace per node at startup and tears them down
at exit via the `netns_rs` crate's RAII guards. The namespace creation sequence
for a node named `n1` is:

+ `NetNs::new("sim_ns_n1")` creates the namespace and returns a guard.
+ `nsi.run(|_| callback(args))` executes the node factory callback inside
  the namespace, so that all sockets and interfaces created during `callback`
  are bound to `n1`'s namespace.
+ TUN/TAP interfaces are created via `tun_tap::Iface`, assigned IP and MAC
  addresses, and brought up inside the namespace.
+ The guard is held for the duration of the simulation; dropping it would
  destroy the namespace and all interfaces within it.

Because each node runs in its own namespace, IP addresses can be reused across
nodes (which simplifies configuration) and no routing rules in the host system
are needed. The host OS sees only the namespace file descriptors, not any of
the virtual interfaces inside.

=== Channel Model

Inter-node communication goes entirely through userspace channel objects rather
than kernel routing. For every directed edge $(A \to B)$ in the topology, the
simulator maintains a `Channel` instance. The channel:

+ *Receives* raw Ethernet frames from node $A$'s TUN write end.
+ *Filters* by destination MAC: frames not addressed to $B$ (unicast to a
  different node or broadcast) are passed through; the MAC filter prevents
  unintended cross-talk on shared virtual media.
+ *Loss injection*: draws a uniform random float; if it is below the configured
  `loss` probability, the frame is discarded. The RNG is seeded per-channel,
  enabling deterministic replay of experiments.
+ *Latency + jitter injection*: the surviving frame is enqueued with an
  arrival timestamp of `now + latency + U(-jitter, +jitter)`. A Tokio background
  task sleeps until that timestamp using `tokio_timerfd::sleep` (which uses the
  kernel `timerfd` interface for accurate sub-millisecond sleeps) and then
  writes the frame to node $B$'s TUN read end.

Channel parameters (`latency`, `loss`, `jitter`) are stored behind an `Arc<RwLock<ChannelParams>>`. The HTTP API and TUI write to this lock; the channel task reads it before enqueuing each frame. Parameter updates therefore take effect immediately for all frames received after the update, with no node restart required.

This model accurately reproduces the first-order statistical properties of a
wireless channel (mean delay, mean loss rate, jitter distribution) without
implementing a physical-layer model. It is appropriate for studying routing
convergence, failover behaviour, and cryptographic handshake timing, which
depend on packet delivery statistics rather than PHY-level propagation effects.

=== Node Lifecycle

The simulator manages node lifecycle as follows:

+ *Creation*: `create_node_from_settings()` runs inside the namespace,
  creates interfaces, and returns a `Box<dyn Node>`. The `Node` trait exposes
  `name()`, `stats()`, `upstream_info()`, and `start()`.

+ *Starting*: `node.start()` spawns one or more Tokio tasks (the control
  loop, the data forwarding loop, and optional background tasks). These tasks
  run on the shared Tokio runtime; no dedicated OS thread is created per node.

+ *Monitoring*: the simulator periodically calls `node.stats()` and
  `node.upstream_info()` to collect metrics for the HTTP API and TUI. These
  calls acquire read locks on the node's internal state.

+ *Shutdown*: the simulator sends a cancellation signal to all node tasks
  via a `CancellationToken`. Tasks observe the cancellation and exit cleanly.
  Namespace guards are then dropped, destroying all virtual interfaces.

=== TUI Dashboard

The terminal TUI (feature: `tui`, built on `ratatui`) provides seven tabs
accessible via number keys:

/ *Metrics*: per-node counters (packets sent/received, loops detected, crypto
  handshakes completed, encryption errors, TAP read/write counts).

/ *Logs*: a live scrolling view of `tracing` log events from all nodes,
  filterable by level.

/ *Nodes*: a table listing each node's name, type, IP address, MAC address,
  namespace, and current status.

/ *Topology*: an ASCII-art representation of the adjacency matrix, showing
  which nodes are connected and in which direction.

/ *Channels*: a table of all directed links with their current `latency`,
  `loss`, and `jitter` values, editable in place via the TUI.

/ *Upstreams*: per-OBU upstream routing state — which RSU each OBU is
  currently routing toward, via which relay, and the score of the cached route.

/ *Registry*: the server's `obu_routes` table — the mapping from OBU virtual
  MAC to `(VANET MAC, RSU UDP address)` used for downstream delivery.

=== HTTP Control API

The HTTP API (feature: `webview`, port 3030) is implemented with the `warp`
framework and exposes three endpoints:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Endpoint*], [*Method*], [*Description*],
    [`/metrics`], [`GET`],
      [Returns a JSON object mapping node name to per-node counter values. Used by the browser dashboard and by experiment analysis scripts.],
    [`/channel/<a>/<b>/`], [`POST`],
      [Updates channel parameters for the directed link from node `<a>` to node `<b>`. Body: `{"latency":"N","loss":"P","jitter":"J"}`. Takes effect immediately for all subsequent frames. Returns the updated parameters.],
    [`/node_info`], [`GET`],
      [Returns topology metadata: node list with types and addresses, adjacency matrix, and current upstream routing state per OBU. Consumed by the browser dashboard.],
  ),
  caption: [Simulator HTTP control API endpoints],
) <tab-http-api>

The API is stateless with respect to the browser: the simulator is the single
source of truth; the browser is a read-only consumer (except for channel
parameter writes via `POST /channel`).

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

