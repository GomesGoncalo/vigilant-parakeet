// ── Chapter 2 — Background <background> ──────────────────────────────────────

= Background <background>

== Vehicular Networks

=== Architecture: OBU and RSU

A vehicular network consists of two classes of node:

- *On-Board Units (OBUs)* are carried by vehicles. They are mobile, battery-
  powered, and connect to the network only while within radio range of other
  nodes.

- *Road-Side Units (RSUs)* are fixed infrastructure nodes deployed at
  intersections or along roadsides. They act as gateways to the wider internet
  and as anchors for the mobile OBUs.

This architecture is standardised under the ETSI ITS (Intelligent Transport
Systems) framework @etsi-its and the IEEE WAVE (Wireless Access in Vehicular
Environments) suite @ieee-wave.

=== Communication Modes

Two communication modes are defined:

- *V2I (Vehicle-to-Infrastructure)*: direct communication between an OBU and a
  nearby RSU.

- *V2V (Vehicle-to-Vehicle)*: direct communication between two OBUs, typically
  used when no RSU is in range or for cooperative awareness.

Multi-hop forwarding allows OBUs beyond direct RSU range to reach the network
through intermediate OBU relays, forming an ad-hoc mesh.

=== Channel Characteristics

Vehicular channels exhibit:

- *High Doppler spread* due to relative velocities up to 200 km/h.
- *Rapid topology change* as vehicles enter and leave radio range within
  seconds.
- *Asymmetric links* where forward and reverse path quality can differ
  significantly.
- *Intermittent connectivity* making connection-oriented protocols impractical
  for many use cases.

These properties motivate the use of lightweight, beacon-based routing rather
than heavy link-state or distance-vector protocols.

== Routing in Vehicular Networks

=== Topology-Based Routing

Classic routing protocols such as DSDV (Destination-Sequenced
Distance-Vector) @dsdv and AODV (Ad-hoc On-Demand Distance Vector) @aodv
were designed for MANETs and have been adapted for VANETs. They maintain
routing tables through periodic or on-demand control messages, which incurs
overhead that can be prohibitive at vehicular densities.

=== Position-Based Routing

Geographic routing such as GPSR (Greedy Perimeter Stateless Routing) @gpsr
exploits GPS coordinates to forward packets toward a destination without
maintaining global topology state. While efficient, it requires accurate and
up-to-date position information and struggles with sparse networks.

=== Beacon / Heartbeat Routing

Simpler approaches used in vehicle-to-infrastructure settings rely on periodic
*heartbeat* or *beacon* messages broadcast by RSUs. OBUs overhearing heartbeats
measure propagation delay and hop count, and use these metrics to select the
best upstream relay toward the nearest RSU. This model is the basis of the
routing protocol implemented in this work (see @sec-routing-protocol).

== Security in Vehicular Networks

The paper by @l3-security-vehicular analyses Layer-3 security threats in
vehicular networks, focusing on routing manipulation attacks. A core finding is
that an adversary controlling one or more intermediate nodes can selectively
drop, replay, or modify heartbeat messages to poison routing tables across a
wide area. The routing protocol implemented in vigilant-parakeet is designed to
reproduce the network model described in that work and serve as a platform for
studying such attacks.

== Simulation Approaches

=== Network Simulators

Traditional network simulators — ns-3 @ns3, OMNET++ @omnetpp, and SUMO
@sumo (for mobility) — provide rich models but require significant configuration
effort, are difficult to extend with custom Rust code, and do not run actual
production-grade node logic.

=== Container and Namespace-Based Simulation

Linux *network namespaces* provide lightweight OS-level isolation of the network
stack. Each namespace has its own routing table, interface list, and firewall
rules, but shares the host kernel. This allows real node binaries to run in
isolation with controlled inter-namespace connectivity, offering a faithful
emulation environment without a full hypervisor.

Tools such as Mininet @mininet popularised this approach for software-defined
networking research. vigilant-parakeet applies the same technique to vehicular
nodes.

=== Rust for Systems Programming

Rust @rust provides memory safety without garbage collection, making it
well-suited for implementing network protocol logic that must be both correct
and efficient. The Tokio @tokio asynchronous runtime allows many nodes to run
concurrently on a single thread pool, enabling dense simulations on commodity
hardware.
