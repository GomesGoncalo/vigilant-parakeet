// ── Chapter 3 — Architecture <architecture> ──────────────────────────────
#import "@preview/fletcher:0.5.7" as fletcher: diagram, node, edge
#import "@preview/chronos:0.2.1"

= Architecture <architecture>

This chapter describes the high-level design of vigilant-parakeet: the crate
decomposition, the three-tier network model, and the key design decisions.

== Workspace Overview

vigilant-parakeet is organised as a Rust Cargo workspace with ten crates
(@fig-workspace-crates). The central principle is strict separation of concerns:
shared protocol code lives in `node_lib` and `common`; node-type behaviour is
encapsulated in `obu_lib`, `rsu_lib`, and `server_lib`; orchestration and
analysis tooling sit in `simulator` and `scripts_tools`; key generation is
provided by the standalone `keygen` utility.

#figure(
  diagram(
    node-stroke: 0.5pt,
    spacing: (18mm, 10mm),
    node((0,0), [`simulator`]),
    node((0,2), [`node`]),
    node((0,4), [`visualization`]),
    node((0,5), [`scripts_tools`]),
    node((0,6), [`keygen`]),
    node((1,4), [Simulator\ HTTP API]),
    node((2,0), [`obu_lib`]),
    node((2,1), [`rsu_lib`]),
    node((2,2), [`server_lib`]),
    node((3,1), [`node_lib`]),
    node((4,1), [`common`]),
    edge((0,0), (2,0), "->"),
    edge((0,0), (2,1), "->"),
    edge((0,0), (2,2), "->"),
    edge((0,2), (2,0), "->"),
    edge((0,2), (2,1), "->"),
    edge((0,2), (2,2), "->"),
    edge((2,0), (3,1), "->"),
    edge((2,1), (3,1), "->"),
    edge((2,2), (3,1), "->"),
    edge((3,1), (4,1), "->"),
    edge((0,4), (1,4), "->"),
    edge((0,6), (3,1), "->"),
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
- *`messages/`* — wire encoders/decoders: `Heartbeat` (30 B: duration + id + hops + source), `HeartbeatReply` (36 B: + sender), `Data`, `KeyExchangeInit`, `KeyExchangeReply`, `PacketType`, `Message`. All fields are big-endian; `TryFrom<&[u8]>` and `Into<Vec<u8>>` are implemented for zero-copy deserialisation.
- *`crypto/`* — full configurable cipher suite: X25519 DH, HKDF, AES-256-GCM / AES-128-GCM / ChaCha20-Poly1305.
- *`data/`* — data-path helpers split by node type (`data::obu`, `data::rsu`).
- *`control/`* — shared routing utilities: `route`, `routing_utils`, `client_cache`, `ReplyType`.
- *`args/`* — `NodeType` enum (`Rsu | Obu | Server`) used by the node binary and simulator.
- *`buffer_pool`* — three-tier pool of reusable `BytesMut` buffers to reduce
  hot-path allocation: SMALL (256 B), MEDIUM (512 B), LARGE (1 500 B), with a
  capacity of 32 buffers per tier stored in a thread-local
  `Mutex<Vec<BytesMut>>`. A caller acquires the smallest tier that fits the
  requested length; on release the buffer is returned to the pool rather than
  dropped.
- *`control::ReplyType`* — discriminates how a processed frame should be
  delivered: `WireFlat` sends the frame out on the VANET device (tier 1),
  while `TapFlat` injects it into the virtual TAP interface (tier 3). This
  enum decouples the control-plane dispatch logic from the physical write path.
- *`Node` trait* — `trait Node: Send + Sync` implemented by `Obu` and `Rsu`. Exposes `as_any()` for runtime downcasting inside the simulator. `Server` is held directly as `Arc<Server>` and does not implement this trait.
- *`test_helpers/`* — `Hub` and `mk_shim_pair()` for non-privileged integration testing.

=== Internal Async Task Architecture

Each node type decomposes its work into a fixed set of Tokio tasks, spawned
when the node starts. The tasks communicate via shared `Arc<RwLock<...>>` state
and `tokio::sync` primitives (channels, notifications, cancellation tokens).
Understanding this task structure is important for reasoning about concurrency
properties and for diagnosing performance under load.

==== OBU Task Structure

An OBU spawns the following Tokio tasks:

/ *VANET receive loop*: reads raw Ethernet frames from the `vanet` TUN
  interface, dispatches on message type (`Heartbeat` → routing update,
  `HeartbeatReply` → forward with `ForwardAction` check,
  `Data::Downstream` → decrypt + write to virtual TAP,
  `KeyExchangeReply` → complete DH exchange), and writes replies back to the
  VANET interface.

/ *TAP transmit loop*: reads frames from the virtual TAP interface (application
  traffic destined for the server), encrypts with the established session key,
  and emits `Data::Upstream` on the VANET interface.

/ *DH rekey timer*: sleeps for `dh_rekey_interval_ms`, then checks whether
  the session key is older than `dh_key_lifetime_ms`. If so, it calls
  `rekey_notify.notify_one()` to wake the rekey task.

/ *DH rekey executor*: waits on `rekey_notify`; when notified, sends a
  `KeyExchangeInit` to the server via the current upstream and marks the key
  store as `Pending`. Also handles the `dh_reply_timeout_ms` timeout:
  if no `KeyExchangeReply` arrives within the timeout, it retries up to the
  configured maximum.

/ *Admin console listener*: accepts TCP connections on `admin_port`, spawns
  a short-lived task per connection that reads line-oriented commands and
  replies.

All tasks share a `CancellationToken`; when the simulator sends a cancellation
signal, each task observes the token at its next yield point and exits cleanly.

==== RSU Task Structure

An RSU spawns:

/ *Heartbeat emitter*: sleeps for `hello_periodicity` ms, increments the
  sequence counter, serialises a `Heartbeat`, and writes it to the VANET
  broadcast address. Updates the `sent` history ring for replay-window
  validation.

/ *VANET receive loop*: dispatches `HeartbeatReply` messages to the routing
  table updater and forwards `Data::Upstream` / `KeyExchangeForward` datagrams
  to the server via the cloud UDP socket.

/ *Cloud receive loop*: reads from the cloud UDP socket; forwards
  `Data::Downstream` and `KeyExchangeResponse` back to the appropriate OBU on
  the VANET interface.

/ *Admin console listener*: as for OBU.

The RSU has no DH or TAP tasks — it is a stateless relay at the session-key
level.

=== `obu_lib` — OBU Node

Implements the full OBU control plane:
- Heartbeat reception, routing table management (~1117-line `Routing`), N-best upstream caching and failover.
- DH key store (`DhKeyStore`): pending → established key lifecycle per server, with retry and expiry.
- TAP session: reads frames from the virtual TAP, encrypts them, and sends them upstream.
- *TCP admin interface* (`admin.rs`): interactive console for inspecting upstream routes, DH session state, and triggering manual re-key (see @sec-admin-console).
- Two network interfaces: `vanet` TAP (tier 1) and `virtual` TAP (tier 3).

Key `ObuParameters` defaults (from `obu_lib::args`):

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Parameter*], [*Default*], [*Purpose*],
    [`hello_history`],          [`10`],          [Max heartbeat entries retained per RSU–sender pair in the routing table],
    [`cached_candidates`],      [`3`],           [Number of N-best upstream candidates stored for fast failover],
    [`dh_rekey_interval_ms`],   [`43 200 000`],  [Proactive re-key interval: 12 hours],
    [`dh_key_lifetime_ms`],     [`86 400 000`],  [Maximum session key age before the OBU forces a new exchange: 24 hours],
    [`dh_reply_timeout_ms`],    [`5 000`],       [Timeout waiting for a `KeyExchangeReply` before retrying],
    [`mtu`],                    [`1 400`],       [Maximum TAP frame size forwarded upstream],
  ),
  caption: [Key `ObuParameters` defaults],
) <tab-obu-params>

=== `rsu_lib` — RSU Node

Implements the RSU control plane:
- Periodic Heartbeat emission and HeartbeatReply processing.
- Client cache (`ClientCache`): tracks OBU VANET MAC → virtual MAC associations.
- *No virtual overlay TAP*: unlike OBUs and the server, RSUs have no decapsulated-traffic interface. They only have a `vanet` TAP (tier 1) and a `cloud` UDP socket (tier 2). They never decrypt or inspect OBU payload.
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

+ *Interface and node creation*: delegates to the `node_factory` module
  (`simulator::node_factory`), which creates the correct set of virtual
  interfaces inside the namespace context and instantiates the appropriate
  library (`obu_lib`, `rsu_lib`, `server_lib`).

+ *Channel management*: each directed link is represented by a `Channel`
  object that applies configurable latency, loss, and jitter entirely in
  userspace. Channel parameters can be updated at runtime without restarting
  any node.

+ *Observability*: exposes an HTTP metrics API (port 3030) and a terminal
  TUI dashboard. The simulator enables observability (metrics, web API and TUI)
  by default to support interactive experiment runs and the native visualization tool.

The simulator architecture is described in full in @implementation.

=== `visualization` — Browser Dashboard

A browser frontend (Yew-based) served from a static HTTP server. It polls the simulator's `/node_info` endpoint on a configurable interval and renders:

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

=== `keygen` — Key-Generation Utility

A standalone binary for generating signing keypairs used in DH message
authentication. It outputs an Ed25519 or ML-DSA-65 seed and its corresponding
verifying key in hexadecimal:

```sh
keygen generate ed25519
keygen generate ml-dsa-65
```

The generated seed is stored in a node's YAML configuration under
`signing_key_seed`, enabling stable, restartable signing identities for PKI
mode (see @sec-trust-models).

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
+ *Loss injection*: draws a uniform random float via `rand::random::<f64>()`
  (unseeded, non-deterministic); if the value is below the configured `loss`
  probability, the frame is discarded. Because the RNG is not seeded, each
  simulation run is statistically independent.
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
  creates interfaces, and returns a `Box<dyn Node>` for OBU and RSU nodes.
  Server nodes are started synchronously during creation and held as
  `Arc<Server>`. The `Node` trait exposes only `as_any()` for runtime
  downcasting.

+ *Starting*: OBU and RSU nodes spawn their Tokio tasks (control loop, data
  forwarding loop, and optional background tasks) when the simulator calls
  their start method on the concrete type. Server nodes begin their async
  tasks during creation via `block_in_place`. All tasks run on the shared
  Tokio runtime; no dedicated OS thread is created per node.

+ *Monitoring*: the simulator collects device and TUN statistics via
  `device.stats()` and `tun.stats()` on the associated interfaces. Node
  routing state (e.g.\ upstream routes) is accessed by downcasting via
  `as_any().downcast_ref::<Obu>()` and calling concrete methods such as
  `cached_upstream_route()`.

+ *Shutdown*: the simulator sends a cancellation signal to all node tasks
  via a `CancellationToken`. Tasks observe the cancellation and exit cleanly.
  Namespace guards are then dropped, destroying all virtual interfaces.

=== TUI Dashboard

The terminal TUI (built on `ratatui`) provides seven tabs
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
framework and exposes the following endpoints:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Endpoint*], [*Method*], [*Description*],
    [`/metrics`], [`GET`],
      [Returns a JSON object mapping node name to per-node counter values. Used by the browser dashboard and by experiment analysis scripts.],
    [`/stats`], [`GET`],
      [Returns per-node device and TUN traffic counters (packets sent/received) aggregated across all interfaces.],
    [`/nodes`], [`GET`],
      [Returns a list of all nodes with their name, type, IP address, and MAC address.],
    [`/node/<name>`], [`GET`],
      [Returns detailed information for a single node identified by `<name>`.],
    [`/channels`], [`GET`],
      [Returns the current channel parameters (`latency`, `loss`, `jitter`) for every directed link in the topology.],
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

This is the central security invariant of the three-tier model and directly
addresses the attacker model of @l3-security-vehicular: a compromised RSU can
observe and replay routing control messages (heartbeats, HeartbeatReplies), but
cannot decrypt or modify OBU payload. The RSU forwards `UpstreamForward` UDP
datagrams to the server without ever possessing the session key. The server is
the sole entity that can decrypt payload, and it is not exposed to the VANET
medium.

=== Separation of Protocol Logic from Orchestration

A recurring problem in simulation-based research is the *fidelity gap*: the
simulator contains a reimplemented approximation of the protocol, and any
divergence between the approximation and the real implementation introduces
potential for false results (@related-work). vigilant-parakeet eliminates this
gap by design: `obu_lib`, `rsu_lib`, and `server_lib` contain the complete
production node implementations, and the `simulator` crate is a thin
orchestration layer that creates namespaces, instantiates these library crates,
and plumbs them together with a channel model. The same library code that runs
in integration tests (`cargo test`) is the code that runs in simulation and on
physical hardware via the `node` binary.

The consequence is that the simulation and production codebases share a single
maintenance surface. A bug fixed in `obu_lib` is fixed everywhere simultaneously.

=== Pure Route Selection

`get_route_to(Some(mac))` is a pure, read-only function.
`select_and_cache_upstream()` is the sole write path for updating the cached
upstream. This design was chosen deliberately to prevent hidden mutations under
`RwLock` read guards — a class of bug that causes data races under Rust's type
system only if the wrong lock type is used. By keeping all routing reads
pure and concentrating all routing writes in a single function,
the routing state is easy to reason about, test in isolation, and profile.

=== Async-First, No Threads per Node

All node I/O runs as Tokio @tokio async tasks on a shared multi-threaded
executor, enabling dense simulations (tens of nodes) on commodity hardware
without per-node OS threads. The choice of Rust and Tokio was motivated by
four properties:

*Memory safety without a garbage collector.* Rust's ownership and borrow
checker eliminates entire classes of memory-safety bugs (buffer overflows,
use-after-free, data races) at compile time, without introducing the latency
jitter of garbage collection. For a timing-sensitive routing evaluation, GC
pauses would be confounding variables.

*Zero-cost abstractions.* Async Rust compiles `async fn` coroutines to
state machines with no dynamic allocation per `.await`. The per-task overhead
of a Tokio task is a single heap allocation for the stack frame — typically
under 256 bytes — making it practical to run dozens of concurrent I/O loops
per node without resource pressure.

*Natural latency injection.* `tokio::time::sleep(duration)` correctly models
a link delay without blocking any OS thread. Tokio's time facilities also
support *mocked time* (via `tokio::time::pause()` and `advance()`), which the
test infrastructure exploits to drive deterministic integration tests at
nanosecond precision without wall-clock waits.

*Ecosystem.* The `tokio`, `bytes`, `mac_address`, `indexmap`, `rand`, and
`warp` crates in the Rust ecosystem provide the building blocks for all major
subsystems without requiring any unsafe code in the node libraries.

=== Instrumentation and default features

To simplify experiment workflows, the simulator ships with observability enabled
by default: the HTTP API, terminal TUI, and the lightweight `stats` counters
are included in normal simulator builds. Library crates (for example
`node_lib` and `common`) keep their `stats` code behind an optional feature so
that downstream consumers can opt in or keep node builds minimal. The
`test_helpers` feature still unlocks in-process hub and TUN shim implementations
used by the integration test suite and remains opt-in for CI and local tests.

=== Test Infrastructure Without Root

The `test_helpers::Hub` and `TokioTun` shim allow full integration tests of
multi-hop routing, encryption, and failover without creating kernel interfaces
or requiring elevated privileges. The core insight is that the routing and
encryption logic does not depend on the mechanism by which frames are sent and
received — only on the interface (the `Device` and `Tun` traits). By providing
trait implementations backed by in-memory channels, the test suite can exercise
the full control plane deterministically and without OS-level side effects.

This design makes the test suite runnable in CI without `sudo`, which is
critical for automated testing in standard container environments (GitHub
Actions, GitLab CI). The simulator itself — which creates real namespaces —
is excluded from CI coverage precisely because it requires privileges, but the
library logic it orchestrates is fully covered.

=== Why Userspace Channel Emulation over `tc-netem`

The Linux kernel's `tc-netem` module (Netlink-configurable network emulator)
provides packet delay, loss, duplication, and corruption in the kernel fast
path. It would be a natural substrate for link emulation. vigilant-parakeet
instead implements a userspace `Channel` object for three reasons:

First, `tc-netem` rules are attached to a *kernel network interface*, which
means they require privileged `ip link` / `tc` invocations to configure. The
simulator would need additional privilege grants or a privileged helper process
for every channel parameter update. The userspace approach updates parameters
via an `Arc<RwLock<ChannelParams>>` with no system call overhead.

Second, `tc-netem` distributes jitter using a pre-compiled kernel distribution
table (loaded via `tc dist`), making runtime distribution changes expensive.
The userspace model makes distribution parameters a simple struct field change.

Third, and most importantly, the userspace channel cooperates with Tokio's
mocked time. `tc-netem` uses the real kernel clock for delay injection; a test
using `tokio::time::pause()` to fast-forward time would not advance `tc-netem`
delays. The userspace `tokio_timerfd::sleep` calls do advance under mocked
Tokio time, enabling sub-millisecond precision in integration tests.

