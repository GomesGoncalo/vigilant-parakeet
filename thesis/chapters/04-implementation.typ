// ── Chapter 4 — Implementation <implementation> ───────────────────────────────

= Implementation <implementation>

== Wire Protocol

=== Message Types

All VANET inter-node communication uses fixed-format binary messages in
`node_lib::messages`:

/ `Heartbeat`: Emitted periodically by RSUs. Carries the RSU MAC address,
  a monotonically increasing sequence number, and a hop counter incremented
  at each relay.

/ `HeartbeatReply`: Sent by an OBU (or relay) in response to a received
  `Heartbeat`. Carries the original sequence number and the replying node's
  MAC. Used to measure round-trip latency.

/ `Data::{Upstream, Downstream}`: Payload wrappers carrying a (potentially
  encrypted) byte buffer with source and destination MAC addresses.

/ `KeyExchangeInit` / `KeyExchangeReply`: 42-byte DH handshake messages
  (see @security for full detail).

/ `Message`: Outer container with a 1-byte type discriminant followed by
  the serialised inner message.

All types implement `TryFrom<&[u8]>` for zero-copy deserialisation and
`Into<Vec<u8>>` for serialisation.

=== Heartbeat Wire Layout

```
┌──────────────────┬────────────┬────────────┐
│  MAC addr (6 B)  │  Seq (4 B) │ Hops (2 B) │
└──────────────────┴────────────┴────────────┘
```

Fields are little-endian. The MAC is the source RSU's hardware address,
used as the primary routing table key.

=== Cloud Protocol Wire Layout

RSU–Server communication uses a separate binary protocol over UDP
(`server_lib::cloud_protocol`):

```
┌──────────────┬──────────┬── payload ──────────────────────────┐
│ MAGIC (2 B)  │ TYPE (1B)│ fields vary by type (see below)     │
│ 0xAB  0xCD   │          │                                     │
└──────────────┴──────────┴─────────────────────────────────────┘
```

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

`get_route_to(Some(target))` in `obu_lib::control::routing` proceeds as:

+ *Direct RSU entries* (zero hops) are preferred unconditionally.

+ Among multi-hop candidates, a composite score is computed:

  $ s = alpha dot t_"avg" + (1 - alpha) dot h $

  where $t_"avg"$ is mean observed RTT, $h$ is advertised hop count, and
  $alpha = 0.7$. Ties broken by MAC lexicographic order.

+ The *cached upstream* is preferred when within a ~10% hysteresis band to
  prevent route flapping.

=== N-Best Candidate Caching

`select_and_cache_upstream(mac)` stores the primary route plus a ranked list
of up to `cached_candidates` (default: 3) alternative next hops.
`failover_cached_upstream()` promotes the head of that list to primary
without recomputing from scratch.

=== Loop Prevention

- *Immediate-bounce guard*: if `pkt.from == next_upstream`, do not forward
  a `HeartbeatReply` back to sender.
- *Sender-loop guard*: if `next_upstream == message.sender()`, drop.

== End-to-End Data Path

The full data path for an OBU sending a packet to an application server
illustrates how the three tiers interact (@fig-data-path):

#figure(
  ```
  OBU                    RSU                    Server
   │                      │                       │
   │ 1. encrypt(payload)  │                       │
   │ 2. Data::Upstream ──►│                       │
   │   (VANET tier 1)     │ 3. UpstreamForward ──►│
   │                      │   (UDP tier 2, 0x02)  │ 4. decrypt(payload)
   │                      │                       │ 5. write to virtual TAP
   │                      │                       │ 6. read reply from TAP
   │                      │                       │ 7. encrypt(reply)
   │                      │◄── DownstreamForward ─│
   │                      │   (UDP tier 2, 0x03)  │
   │◄── Data::Downstream ─│                       │
   │   (VANET tier 1)     │                       │
   │ 8. decrypt(reply)    │                       │
   │ 9. write to virt TAP │                       │
  ```,
  caption: [End-to-end data path across all three tiers],
) <fig-data-path>

== Simulator Orchestration

=== Network Namespace Setup

For each node the simulator:

+ Creates a namespace: `ip netns add sim_ns_<name>`.
+ Creates a `veth` pair; moves one end into the namespace.
+ Assigns the configured IP to the in-namespace interface.
+ Applies `tc netem` on the host-side veth to enforce per-link
  `latency` and `loss` from the topology YAML.

=== node_factory

`create_node_from_settings()` creates the correct set of interfaces and
node instance inside the namespace context:

- *OBU*: `vanet` TAP + `virtual` TAP → `obu_lib::create_with_vdev(args, tun, device, name)`
- *RSU*: `vanet` TAP + `cloud` TAP (UDP socket bound here) → `rsu_lib::create_with_vdev(args, device, name)`
- *Server*: `virtual` TAP + `cloud` TAP (UDP socket) → `Server::new(...).with_tun(tun)`, `server.start()` called immediately via `block_in_place`

=== HTTP Control API (feature: `webview`)

| Endpoint | Method | Description |
|---|---|---|
| `GET /metrics` | — | JSON per-node counters |
| `POST /channel/<a>/<b>/` | `{"latency":"N","loss":"P"}` | Update `tc netem` rules at runtime |
| `GET /node_info` | — | Topology and upstream state for visualization |

== Test Infrastructure <sec-test-infrastructure>

=== The `Hub` Helper

`node_lib::test_helpers::hub::Hub` is an in-process programmable switch:
- Per-link latency injection (`tokio::time::sleep`) and loss injection (seeded RNG).
- Watch hooks — `Sender<Frame>` channels — for traffic assertions without modifying production code.

=== TUN Shim

`common::Tun::new_shim()` returns a `(TokioTun, FrameChannel)` pair backed
by Tokio channels. Tests inject frames into nodes and assert on outputs with
zero OS-level side effects and no root privileges required.

=== Integration Test Coverage

`node_lib/tests/` contains nine test files:
`integration_flow`, `integration_topology`, `integration_two_hop`,
`integration_encryption`, `integration_broadcast_encryption`,
`integration_failover_send_error`, `integration_loop_repro`,
`integration_latency_measurement_mocked_time`, `routing_integration`.

`server_lib/tests/integration_encryption.rs` covers the server-side
DH key exchange and decryption path end-to-end.

