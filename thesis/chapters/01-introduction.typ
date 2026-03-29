// ── Chapter 1 — Introduction ──────────────────────────────────────────────────

= Introduction

== Motivation

Vehicular networks — also known as VANETs (Vehicular Ad-hoc Networks) — are a
class of mobile ad-hoc network in which vehicles and roadside infrastructure
communicate to improve road safety, traffic efficiency, and passenger
experience. The combination of high node mobility, intermittent connectivity,
and safety-critical applications makes routing a particularly demanding problem
in this domain.

Existing simulation tools either require costly proprietary hardware, operate
at the application layer only, or fail to model the per-link latency and packet
loss that characterise real vehicular channels. This thesis addresses that gap
by presenting *vigilant-parakeet*: a Linux-native, open-source simulator that
runs real Layer-3 node logic inside isolated network namespaces, allowing
researchers to study routing protocols under controlled but realistic network
conditions.

The project was originally motivated by the vehicular-network security work of
@l3-security-vehicular, whose routing model forms the basis of the protocol
implemented here.

== Research Questions

This thesis investigates the following questions:

+ Can a single-machine Linux simulator faithfully reproduce the routing
  dynamics of a multi-hop vehicular network without specialised hardware?

+ How does the choice of routing metric (latency vs. hop count) affect
  convergence time and route stability under varying loss conditions?

+ What is the overhead introduced by N-best candidate caching on memory and
  CPU, and how much does it reduce route-restoration latency after a next-hop
  failure?

== Contributions

The primary contributions of this work are:

- *A modular Rust simulator* (`vigilant-parakeet`) built as a Cargo workspace,
  separating shared protocol logic (`node_lib`) from node-type implementations
  (`obu_lib`, `rsu_lib`) and the orchestration layer (`simulator`).

- *A heartbeat-based routing protocol* with latency-aware metric computation,
  N-best upstream candidate caching, and fast failover.

- *An in-process test hub* (`node_lib::test_helpers::hub::Hub`) enabling
  deterministic, reproducible integration testing without root privileges or
  physical network devices.

- *A browser-based visualisation dashboard* consuming a real-time HTTP metrics
  API exposed by the simulator.

- *An empirical evaluation* of routing behaviour across a range of topologies,
  latency profiles, and packet-loss rates.

== Thesis Structure

The remainder of this thesis is organised as follows.

- @background reviews vehicular networking concepts, relevant routing
  protocols, and prior simulation approaches.

- @architecture describes the high-level design of vigilant-parakeet and the
  rationale behind its crate decomposition.

- @implementation details the implementation of the routing protocol, the
  simulator, the test infrastructure, and the visualisation layer.

- @security reviews the security architecture: DH key exchange, key
  derivation, configurable cipher suite, and the DH key store lifecycle.

- @evaluation presents the experimental setup and results.

- @conclusion summarises the findings, reflects on limitations, and proposes
  directions for future work.
