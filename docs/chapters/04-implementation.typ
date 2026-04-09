// ── Chapter 4 — Implementation <implementation> ───────────────────────────────
#import "@preview/fletcher:0.5.7" as fletcher: diagram, node, edge
#import "@preview/chronos:0.2.1"

= Implementation <implementation>

== Wire Protocol

=== Message Types

All VANET inter-node communication uses fixed-format binary messages in
`node_lib::messages`:

/ `Heartbeat`: Emitted periodically by RSUs. Carries the RSU's uptime at
  emission time (`duration`), a monotonically increasing sequence number
  (`id`), a hop counter incremented by each relay, and the RSU MAC address
  (`source`).

/ `HeartbeatReply`: Sent by an OBU (or relay) in response to a received
  `Heartbeat`. Copies `duration`, `id`, `hops`, and `source` from the
  original Heartbeat, and appends the replying node's MAC as `sender`. The
  RSU receiving the reply computes round-trip latency as
  `now_uptime − duration`.

/ `Data::{Upstream, Downstream}`: Payload wrappers carrying a (potentially
  encrypted) byte buffer with source and destination MAC addresses.

/ `KeyExchangeInit` / `KeyExchangeReply`: Variable-length DH/KEM handshake
  messages. Size depends on the configured algorithm: 45 bytes unsigned for
  X25519, 1197 bytes for ML-KEM-768 Init, or 1101 bytes for ML-KEM-768 Reply.
  An optional signed extension appends the signing public key and signature,
  adding 101 bytes for Ed25519 or up to 5266 bytes for ML-DSA-65
  (see @security for full wire format detail).

/ `Message`: Outer container with a 1-byte type discriminant followed by
  the serialised inner message.

All types implement `TryFrom<&[u8]>` for zero-copy deserialisation and
`Into<Vec<u8>>` for serialisation.

=== Heartbeat Wire Layout

#figure(
  table(
    columns: (auto, auto, auto, auto),
    align: (center, center, center, center),
    [`duration (16 B)`], [`id (4 B)`], [`hops (4 B)`], [`source (6 B)`],
  ),
  caption: [Heartbeat wire format (30 bytes, big-endian)],
)

All fields are big-endian. `duration` is the sender's uptime in milliseconds
at emission time (16-byte BE integer), used by receivers to timestamp the
arrival and compute round-trip latency. `id` is the monotonically increasing
sequence number. `hops` is incremented by each relay on the forward path.
`source` is the RSU's MAC address, used as the primary routing table key.

=== Cloud Protocol Wire Layout

RSU–Server communication uses a separate binary protocol over UDP
(`server_lib::cloud_protocol`):

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (center, center, left),
    [`MAGIC (2 B)` \ `0xAB 0xCD`], [`TYPE (1 B)`],
    [payload (fields vary by type — see @tab-cloud-protocol)],
  ),
  caption: [Cloud protocol message header],
)

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Type byte*], [*Message*], [*Fields after MAGIC+TYPE*],
    [`0x01`], [`RegistrationMessage`], [`RSU_MAC (6B)`, `OBU_COUNT (2B)`, `OBU_MAC×N`],
    [`0x02`], [`UpstreamForward`], [`RSU_MAC (6B)`, `OBU_SRC_MAC (6B)`, `PAYLOAD`],
    [`0x03`], [`DownstreamForward`], [`OBU_DST_MAC (6B)`, `ORIGIN_MAC (6B)`, `PAYLOAD`],
    [`0x04`], [`KeyExchangeForward`], [`RSU_MAC (6B)`, `CONTROL_PAYLOAD`],
    [`0x05`], [`KeyExchangeResponse`], [`OBU_DST_MAC (6B)`, `RESPONSE_PAYLOAD`],
  ),
  caption: [Cloud protocol message types],
) <tab-cloud-protocol>

== Routing Protocol <sec-routing-protocol>

=== Heartbeat Emission (RSU)

An RSU emits a `Heartbeat` every `hello_periodicity` milliseconds to the
broadcast MAC address `ff:ff:ff:ff:ff:ff`. The sequence number increments
with each emission. The RSU is stateless with respect to upstream routing.

=== Heartbeat Reception and Route Building (OBU)

When an OBU receives a `Heartbeat`:

+ It records an entry in its routing table keyed by `(rsu_mac, seq_id)`,
  storing `pkt.from`, hop count, and arrival timestamp. The table is an
  `IndexMap` bounded by `hello_history`, so stale entries are evicted
  automatically.

+ If the OBU has downstream nodes, it increments the hop counter and
  rebroadcasts.

+ It emits a `HeartbeatReply` toward the RSU.

=== Route Selection Metric

The routing protocol uses a composite latency/hop-count metric implemented in
`obu_lib::control::routing`. The original min+mean metric was extended with
RSSI-aware selection and a stronger hysteresis band to reduce RSU flapping under
time-varying channels and mobility. The goal was to make selection policies more
expressive while remaining easy to reason about and test.

The latency metric was sensitive to single-sample minima under high-variance
channels (e.g., small-scale fading) and to transient packet loss, which in
practice manifested as RSU flapping when vehicles moved through fading dips or
when probe timing coincided with brief congestion. The extensions described
below address these issues.

*Latency-based scoring:* when latency measurements are available, each candidate
next-hop m is scored by the composite metric

`s_m = mu_m^min + overline(mu)_m`

where `mu_m^min` is the minimum observed round-trip latency to m (microseconds)
and `overline(mu)_m` is the mean latency across recorded observations. Combining
minimum and mean penalises candidates that are occasionally fast but highly
variable while rewarding consistently low-latency paths. The metric requires at
least two observations before a decision is made; candidates with fewer samples
fall back to hop-count comparison.

*RSSI-aware selection:* when an RSSI table is available (injected by the
simulator's fading model or a hardware radio driver), RSU selection in
`select_and_cache_upstream()` switches to a signal-strength gate:

- The cached upstream is *retained* unless the incoming RSU presents an RSSI at
  least 3 dB stronger than the cached upstream's last measurement. A 3 dB
  margin corresponds to approximately 40% closer in free-space path loss and is
  a standard hysteresis criterion in cellular handover.
- If the cached upstream's RSSI entry has gone stale, the incoming RSU is
  promoted immediately.
- Without an RSSI table, `select_and_cache_upstream()` falls back to a
  heartbeat reception-ratio comparison with a 30% margin.

*Hysteresis and flapping prevention:* the cached upstream is protected by a
30% latency hysteresis band. Via the `get_route_to()` path, a switch occurs only
when the new candidate's latency score is at least 30% lower than the cached
upstream's, or when it strictly reduces hop count. This prevents oscillation in
mobility-plus-fading scenarios where RTT samples are noisy but the mean link
quality is stable.

*N-best caching and failover:* on each primary-route update the top N candidates
(default N=3) are cached in rank order. `failover_cached_upstream()` promotes
the head of this list on send errors or timeouts, making failover O(1) and
avoiding an immediate heartbeat cycle for route repair.

Implementation notes:
- `get_route_to(Some(target))` is a pure function reading routing state under a read lock.
- `select_and_cache_upstream()` performs the single write to update the cached upstream and rebuild the N-best list.

Unit and integration tests in `obu_lib` exercise these mechanisms with synthetic
channels providing controlled jitter and fading, verifying the 3 dB RSSI gate
policy and that the 30% hysteresis band prevents pathological oscillation. The
dashboard and `/node_info` endpoints expose the scoring components (raw
latencies, smoothed RSSI samples, normalised scores) for post-hoc analysis.

=== Loop Prevention

HeartbeatReply forwarding is controlled by a `ForwardAction` enum computed at
each relay OBU before any I/O takes place:

/ `SkipForward`: Drop the reply and skip forwarding. Triggered when
  `pkt.from == next_upstream` (immediate-bounce guard): sending the reply back
  to its origin would create a two-node oscillation. The relay still updates the
  route cache before returning, so the routing table stays fresh even for bounced
  replies. This variant also covers the case where no upstream is known, because
  the caller returns early before `decide_reply_forward` is reached.

/ `Forward`: Normal case. The reply is forwarded to `next_upstream`, which
  differs from both `pkt.from` and `message.sender()`.

/ `ForwardCached`: Triggered by a mutual-loop race: `next_upstream == sender`,
  meaning two peer OBUs each recorded the other as next_upstream for the same
  Heartbeat sequence. The relay searches for a safe alternative upstream —
  first the globally cached upstream from a prior sequence, then any other
  sequence entry for this RSU, then the RSU source directly — and forwards via
  that alternative. If no safe alternative exists, the packet is dropped with a
  `"loop detected"` error.

The `ForwardAction` calculation is pure (reads routing state under a read lock)
and is performed before acquiring any write lock, keeping the hot-path lock
scope small.

=== Replay Detection

HeartbeatReply messages carry the original Heartbeat sequence number back to
the RSU so that the RSU can compute round-trip latency and update routing
metrics. Without replay protection, an attacker that captures a legitimate
HeartbeatReply can re-inject it later to make the RSU believe a routing path
is fresher or lower-latency than it really is, maintaining stale routing
entries and manipulating route selection.

vigilant-parakeet implements a *per-sender sliding receive window* on
HeartbeatReply sequence numbers at the RSU (`rsu_lib::control::routing::ReplayWindow`),
following the same design as IPsec AH @ipsec-ah:

- Each `(RSU, sender MAC)` pair has its own independent `ReplayWindow`.
- The window state is a `(last_seq: u32, window: u64)` pair. `last_seq` is
  the highest accepted sequence number; `window` is a 64-bit bitmask where
  bit $i$ is set when sequence number `last_seq - i` has been accepted.
- The window width is 64 sequence numbers. A reply with sequence number `seq`
  is accepted if and only if `seq > last_seq - 64` and `seq` has not been
  accepted before.
- On acceptance of a new highest sequence number, the bitmask is left-shifted
  by the advance and bit 0 is set for the new entry.
- Replies with `seq <= last_seq - 64` (outside the window) are silently dropped.
- Replies with a `seq` already recorded in the bitmask are silently dropped as
  duplicates.

A subtle additional guard prevents a *window-poisoning attack*: before
`check_and_update` is called, the RSU verifies that `seq` appears in its own
sent-heartbeat history. Without this guard, an attacker could forge a reply
with `seq = u32::MAX`, advance `last_seq` to `u32::MAX`, and cause all
subsequent legitimate replies from that sender to fall outside the window,
effectively denying routing updates from a benign OBU. The sequence wraparound
case (`u32` overflow) is handled by clearing all window state when the RSU
detects that its own sequence counter has wrapped.

The `ReplayWindow` is unit-tested in `rsu_lib/src/control/routing.rs` via
`replay_window_tests` (same-sequence rejection, outside-window rejection) and
integration-tested via `replay_integration_tests` (replayed reply does not
insert duplicate route, forged large ID does not poison window).

== End-to-End Data Path

The full data path for an OBU sending a packet to an application server
illustrates how the three tiers interact (see figure).

`Data::Upstream` and `Data::Downstream` are the VANET payload wrappers used
on the tier-1 medium. They are wrapped inside the outer `Message` container
(1-byte type discriminant), and their own layout depends on direction:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Variant*], [*Header fields*], [*Followed by*],
    [`Upstream` (`0x00`)],   [`origin (6 B)`],              [AEAD-encrypted payload (nonce + ciphertext + auth tag)],
    [`Downstream` (`0x01`)], [`origin (6 B)` + `dst (6 B)`],[AEAD-encrypted payload],
  ),
  caption: [`Data` wire format inside the `Message` container; no length prefix — the payload occupies all remaining bytes],
) <tab-data-wire>

#figure(
  scale(60%, reflow: true, chronos.diagram({
    import chronos: *
    _par("OBU")
    _par("RSU")
    _par("Server")
    _seq("OBU", "OBU", comment: "1. encrypt(payload)")
    _seq("OBU", "RSU", comment: "Data::Upstream")
    _seq("RSU", "Server", comment: "UpstreamForward (0x02)")
    _seq("Server", "Server", comment: "decrypt/TAP/encrypt")
    _seq("Server", "RSU", comment: "DownstreamForward (0x03)")
    _seq("RSU", "OBU", comment: "Data::Downstream")
    _seq("OBU", "OBU", comment: "decrypt reply; write TAP")
  }, width: 150mm)),
  caption: [End-to-end data path across all three tiers],
) <fig-data-path>

== Simulator Orchestration

=== Network Namespace Setup

For each node the simulator creates an isolated network namespace using
`netns_rs::NetNs::new("sim_ns_<name>")` and executes the node factory
callback inside it via `nsi.run(|_| callback(...))`, giving each node its
own independent network stack.

Per-link channel conditions (latency, loss, jitter) are simulated entirely
in userspace by `simulator::channel::Channel` — no kernel `tc netem` rules
are involved. Each directed link is backed by a `Channel` instance that:

+ *Filters* incoming frames by MAC address (unicast to this node or broadcast).
+ *Drops* packets probabilistically when `rand::random::<f64>() < loss`.
+ *Enqueues* surviving packets with their arrival timestamp and forwards them
  to the destination TUN interface after sleeping for
  `latency ± jitter` using `tokio_timerfd::sleep`, implemented as a
  background Tokio task.

Channel parameters can be updated at runtime (taking effect immediately for
new packets) via the HTTP API or TUI, without restarting any node.

=== `node_factory` Module

`simulator::node_factory::create_node_from_settings()` creates the correct
set of interfaces and node instance inside the namespace context:

- *OBU*: `vanet` TAP + `virtual` TAP → `obu_lib::create_with_vdev(args, tun, device, name)`
- *RSU*: `vanet` TAP + `cloud` TAP (UDP socket bound here) → `rsu_lib::create_with_vdev(args, device, name)`
- *Server*: `virtual` TAP + `cloud` TAP (UDP socket) → `Server::new(...).with_tun(tun)`, `server.start()` called immediately via `block_in_place`

=== HTTP Control API (feature: `webview`)

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Endpoint*], [*Method*], [*Description*],
    [`/metrics`], [`GET`], [JSON per-node counter values],
    [`/stats`], [`GET`], [Per-node device and TUN traffic counters],
    [`/nodes`], [`GET`], [Node list with name, type, IP, and MAC],
    [`/node/<name>`], [`GET`], [Detailed info for a single node],
    [`/channels`], [`GET`], [Current channel parameters for all directed links],
    [`/channel/<a>/<b>/`], [`POST`], [`{"latency":"N","loss":"P","jitter":"J"}` — update per-link channel parameters at runtime],
    [`/node_info`], [`GET`], [Topology and upstream routing state for the browser dashboard],
  ),
  caption: [Simulator HTTP control API endpoints],
) <tab-http-api-impl>

== TCP Admin Console <sec-admin-console>

Each node type exposes a TCP admin interface bound to `127.0.0.1:<admin_port>`
inside its network namespace (default port: 9000). The interface is a
line-oriented text protocol, making it reachable via standard tools without
any additional client software:

```text
ip netns exec <node_ns> nc 127.0.0.1 9000
ip netns exec <node_ns> telnet 127.0.0.1 9000
```

The `admin_port` key in each node's YAML configuration selects the listening
port. Multiple concurrent connections are accepted; each session is
independent. All responses use CRLF line endings for terminal compatibility.

Because the bind address is inside the network namespace, the admin interface
is not reachable from outside the simulation host without explicit namespace
entry — it serves as a local operations channel rather than a network-exposed
management API.

=== OBU Admin Commands

#figure(
  table(
    columns: (auto, 1fr),
    align: (left, left),
    [*Command*], [*Description*],
    [`info`],    [Node identity (name, MAC), current upstream next-hop, and DH session status.],
    [`session`], [Active DH session: `key_id`, age since establishment, and whether a re-key is in progress.],
    [`routes`],  [Cached upstream candidates in priority order (primary + fallback entries).],
    [`rekey`],   [Immediately clears the current DH session and triggers a new key exchange. Useful for manual session rotation or debugging.],
    [`quit`],    [Close the admin connection.],
  ),
  caption: [OBU admin console commands],
)

=== RSU Admin Commands

#figure(
  table(
    columns: (auto, 1fr),
    align: (left, left),
    [*Command*], [*Description*],
    [`info`],    [Node identity (name, MAC), number of known OBU clients, and VANET route count.],
    [`clients`], [Table of known OBU clients: OBU VANET MAC and the next-hop MAC through which the RSU reached each OBU.],
    [`routes`],  [VANET next-hop routing table: next-hop MAC, hop count, and observed latency.],
    [`quit`],    [Close the admin connection.],
  ),
  caption: [RSU admin console commands],
)

=== Server Admin Commands

#figure(
  table(
    columns: (auto, 1fr),
    align: (left, left),
    [*Command*], [*Description*],
    [`sessions`],       [Lists all active DH sessions: OBU VANET MAC, `key_id`, and session age.],
    [`revoke <mac>`],   [Terminates the DH session for the specified OBU. Clears the server-side key and sends a signed `SessionTerminated` message via the RSU to the OBU, which immediately initiates a fresh key exchange (see @sec-session-revocation).],
    [`routes`],         [OBU routing table: virtual TAP MAC, VANET MAC, and the RSU UDP address used for downstream delivery.],
    [`registry`],       [RSU registration table: RSU MAC mapped to its currently associated OBU MACs.],
    [`allowlist`],      [PKI signing allowlist: OBU VANET MACs and their pre-registered verifying keys (empty if PKI mode is not configured).],
    [`quit`],           [Close the admin connection.],
  ),
  caption: [Server admin console commands],
)

== Visualisation Dashboard <sec-visualization>

The browser-based visualisation dashboard (`visualization/`) is a
Yew/WebAssembly application compiled to WASM and served alongside the simulator
HTTP API. It provides a live read-only view of simulation state without
modifying any node behaviour, making it suitable for demonstration and
monitoring without experimental side effects.

=== Technology Stack

The frontend is written in Rust using the *Yew* framework — a component-based
web UI library analogous to React, but compiled from Rust to WebAssembly via
`wasm-bindgen`. Building and serving the dashboard requires `trunk`, a Rust
WASM bundler:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk

cd visualization
trunk build --release   # produces dist/ with index.html + wasm bundle
trunk serve             # dev server with live reload
```

The Yew component model maps directly onto the simulator's data model:
`NodeState`, `ChannelState`, and `UpstreamState` structs are shared with the
HTTP API layer and serialised as JSON. The dashboard fetches the `/node_info`
endpoint on a configurable polling interval (default 1 second) and triggers
a Yew re-render on each update.

=== Dashboard Components

The dashboard renders three primary views:

*Topology graph*: an SVG force-directed graph showing nodes (OBUs in one colour,
RSUs in another, server in a third) connected by directed edges representing
the current topology. Each edge label shows the configured channel latency and
loss for that directed link. Clicking a node opens an info panel showing its
IP address, MAC address, current upstream route, and DH session status.

*Traffic counters*: a table updated on each poll showing per-node packet
counters (`frames_sent`, `frames_received`, `encrypted_frames`, `tap_writes`)
from the `/metrics` endpoint. Counters are displayed as absolute values and
as per-second deltas computed from successive polls, giving an at-a-glance
throughput view.

*Upstream routing state*: a per-OBU panel showing the currently selected
upstream relay MAC and the N-best candidate list. When a failover occurs
(primary candidate promoted to head, new head selected), the UI highlights
the changed entry for one rendering cycle. This makes the failover mechanism
observable in real time during manual experiments.

Additional visualization enhancements

The Leaflet map tab implements smooth marker animation using a requestAnimationFrame
(rAF) driven interpolation between successive `/node_info` polls. Each OBU/RSU
marker tracks the last known position, the most recent position, and a target
timestamp. On each rAF tick the renderer computes a time-aligned interpolated
position and calls `marker.setLatLng(...)`, providing visually smooth motion
independent of the polling frequency. This reduces visual jitter when the
poll interval is coarse relative to vehicle movement.

Marker rendering is optimised for large node counts: custom icon bitmaps are
cached and reused; marker layers are grouped by node type; and directional
routing arrows are rendered using a lightweight canvas overlay (arrow polylines
with simple arrowheads) rather than heavyweight SVG per-edge geometry. Edge
colour encodes a composite health metric (green=good, amber=degraded, red=bad)
derived from channel latency and loss, enabling at-a-glance topology health
assessment.

For performance-sensitive paths the dashboard bypasses the Yew/WASM render
cycle and issues a native JavaScript `fetch('/node_info')` to obtain node
positions and routing state. This JS-native polling path updates the Leaflet
layers directly and only manipulates the DOM via minimal imperative calls,
reducing WASM round-trips and improving responsiveness on lower-power clients.

The map tab filters server and cloud nodes from the display by default, and
re-centres the viewport on visible vehicular nodes when the tab is activated.
Tooltips and a per-node popup provide the current upstream route and the
N-best candidate list, making the map a useful surface for observing routing
convergence and failover events during experiments.

=== Architectural Separation

The dashboard is architecturally separate from the simulator: it communicates
exclusively via the HTTP API and makes no assumptions about the simulator's
internal implementation. The only shared artefact is the JSON schema of the
API responses, which is defined as Rust structs in `simulator/src/webview.rs`
and serialised via `serde_json`. This separation means the dashboard can be
updated or replaced without modifying the simulator, and the simulator can
run without a browser client.

The WebAssembly target is fully separate from the simulator's compilation:
`cargo build --workspace` does not include the `visualization` crate (it
requires the `wasm32-unknown-unknown` target and `trunk`). CI builds the WASM
artifact in a separate step from the native simulator.

== Simulator Configuration <sec-configuration>

The simulator is configured via two layers of YAML files: a top-level
*topology file* describing the simulation scenario, and per-node *node
configuration files* specifying each node's parameters.

=== Topology File Format

The topology file controls scenario structure:

```yaml
# simulator.yaml
nodes:
  rsu1:
    config_path: rsu1.yaml
  obu1:
    config_path: obu1.yaml
  obu2:
    config_path: obu2.yaml
  server:
    config_path: server.yaml

topology:
  rsu1:
    obu1:
      latency: 10    # ms
      loss: 0.0      # probability 0.0–1.0
      jitter: 2      # ms
    obu2:
      latency: 50
      loss: 0.05
      jitter: 5
  obu1:
    rsu1:
      latency: 10
      loss: 0.0
      jitter: 2
    obu2:
      latency: 5
      loss: 0.0
      jitter: 1
```

Each entry under `topology` is a directed edge with `latency` (ms),
`loss` (probability 0.0–1.0), and `jitter` (ms half-range). Edges are
unidirectional: `rsu1 → obu1` and `obu1 → rsu1` are configured
separately, allowing asymmetric link conditions.

The YAML values are parsed at startup and used to initialise
`Channel` instances for each directed edge. Channel parameters can be
updated at runtime (see @sec-routing-protocol) via the HTTP API without
changing or reloading the file.

=== Node Configuration File Format

Each node has an individual YAML file specifying its type and operational
parameters:

```yaml
# obu1.yaml
node_type: Obu
ip: 10.0.0.2            # VANET IP address
admin_port: 9001        # TCP admin console port

# Routing parameters
hello_history: 10       # heartbeat entries per peer
cached_candidates: 3    # N-best upstream candidates

# DH key exchange parameters
dh_rekey_interval_ms: 43200000   # 12 hours
dh_key_lifetime_ms:   86400000   # 24 hours
dh_reply_timeout_ms:  5000       # 5 seconds

# Crypto configuration
cipher: aes-256-gcm     # aes-256-gcm | aes-128-gcm | chacha20-poly1305
dh_group: x25519        # x25519 | ml-kem-768
mtu: 1400               # max TAP frame size

# Optional: DH message authentication
# enable_dh_signatures: true
# signing_algorithm: ed25519       # ed25519 | ml-dsa-65
# signing_key_seed: "<64-hex>"
# server_signing_pubkey: "<hex>"
```

```yaml
# rsu1.yaml
node_type: Rsu
ip: 10.0.0.1
hello_periodicity: 500  # ms between Heartbeats
hello_history: 10
admin_port: 9000
```

```yaml
# server.yaml
node_type: Server
ip: 172.0.0.1           # cloud-tier IP
admin_port: 9100
# Optional: signing allowlist for PKI mode
# dh_signing_allowlist:
#   "AA:BB:CC:DD:EE:FF": "<hex verifying key>"
```

The configuration system allows a single experiment directory to contain
multiple topology files targeting different channel conditions or node counts,
with all nodes sharing the same per-node YAML. This supports systematic
parameter sweeps: the `scripts_tools::generate-pairs` command generates
topology matrices across a range of latency and loss values for automated
batch experiments.

== Test Infrastructure <sec-test-infrastructure>

=== The `Hub` Helper

`node_lib::test_helpers::hub::Hub` is an in-process programmable switch backed
by raw UNIX socket pairs (one pair per attached node). Its main characteristics
are:

- *Frame delivery*: the Hub's poll loop calls `libc::recv(fd, MSG_DONTWAIT)` on
  each node's socket in a tight loop with 100 µs sleeps, forwarding each frame
  to all other attached nodes after an optional per-hop delay via
  `tokio::time::sleep`. Because this uses Tokio's time primitives, the delay
  respects mocked time (via `tokio::time::pause()` / `advance()`), making
  deterministic latency tests possible without real wall-clock waits.

- *The `HubCheck` trait*: tests implement `HubCheck` (with a single
  `on_packet(from_idx, data)` method) and attach the checker as a watch hook.
  The Hub calls the hook for every forwarded frame, allowing precise per-packet
  assertions without any changes to production node code.

- *Loss injection*: per-link loss is applied with a seeded
  `rand::rngs::SmallRng`, giving deterministic packet-drop sequences across
  test runs.

- *`UpstreamExpectation`*: a helper that wraps a `tokio::sync::oneshot::Receiver`
  and resolves when a matching frame has been observed, enabling `await`-based
  test assertions.

=== TUN Shim

`common::Tun::new_shim()` returns a `(TokioTun, FrameChannel)` pair backed
by Tokio channels. Tests inject frames into nodes and assert on outputs with
zero OS-level side effects and no root privileges required.

=== Integration Test Coverage

`node_lib/tests/` contains nine integration test files; each targets a
specific protocol property:

/ `integration_flow`: Verifies that the base `handle_messages` dispatch table
  correctly routes incoming frames to the TUN device or VANET device according
  to message type.

/ `integration_topology`: Creates a minimal RSU + OBU topology and asserts that
  the OBU discovers a valid upstream route to the RSU after receiving its first
  Heartbeat.

/ `integration_two_hop`: Places two OBUs in a chain (OBU₁ — OBU₂ — RSU) and
  confirms that OBU₂ prefers the two-hop path through OBU₁ when the direct link
  carries higher simulated latency.

/ `integration_encryption`: Uses mocked time to drive the full DH key exchange
  between an OBU and the server, then asserts that TAP frames sent by the OBU
  arrive at the server decrypted. Verifies that an eavesdropping relay OBU sees
  only ciphertext.

/ `integration_broadcast_encryption`: Confirms that broadcast traffic sent by
  the server remains opaque to intermediate relay OBUs that do not hold the
  session key.

/ `integration_failover_send_error`: Injects a send error on the primary
  upstream link and asserts that the OBU immediately promotes the next N-best
  candidate, restoring end-to-end frame delivery without waiting for a new
  Heartbeat cycle.

/ `integration_loop_repro`: Reproduces a historical loop condition in a
  diamond topology and asserts that the `ForwardAction` guards prevent any
  HeartbeatReply from being forwarded indefinitely.

/ `integration_latency_measurement_mocked_time`: Uses `tokio::time::advance`
  to simulate precise round-trip intervals and asserts that the RSU computes
  the correct RTT from the `duration` field of the HeartbeatReply.

/ `routing_integration`: End-to-end routing-table convergence test: verifies
  that after several Heartbeat cycles the routing table contains the expected
  next-hop entries and that the hysteresis threshold prevents spurious route
  changes under small latency perturbations.

`server_lib/tests/integration_encryption.rs` covers the server-side
DH key exchange and decryption path end-to-end, including ML-KEM-768 and
X25519 variants.

