// ── Related Work <related-work> ───────────────────────────────────────────────

= Related Work <related-work>

This chapter surveys work related to vigilant-parakeet across three dimensions:
simulation and emulation platforms for vehicular and ad-hoc networks, routing
protocol implementations, and security frameworks with particular attention to
post-quantum migration challenges in V2X environments. The chapter concludes
with a consolidated comparison that positions vigilant-parakeet within this
landscape.

== VANET Simulation and Emulation Platforms

A central design choice for any VANET research tool is the trade-off between
*model fidelity* (how faithfully the simulator reproduces real hardware behaviour)
and *execution fidelity* (whether the actual production code runs, or a synthetic
reimplementation). This distinction drives almost all architectural differences
between the tools reviewed here.

=== Coupled Mobility and Network Simulators

The dominant research methodology for VANET evaluation combines a *road traffic
simulator* with a *network event simulator*, coupling them so that vehicle
positions and speeds from the traffic side drive node placement in the network
side.

*VEINS* (Vehicles in Network Simulation) @veins is the most widely used
vehicular simulation framework. It couples OMNeT++ @omnetpp with SUMO @sumo
via the TraCI (Traffic Control Interface) socket protocol. SUMO computes
vehicle dynamics at each simulation step; the TraCI bridge pushes position
and velocity updates to OMNeT++ nodes. VEINS implements the IEEE 802.11p
(WAVE) physical and MAC layers in C++, using the Two-Ray interference model
or the Nakagami-m fading model to compute received signal strength and
packet error rates as a function of inter-vehicle distance, obstacle geometry,
and mobility. The bidirectional coupling means that a VANET application can
react to the simulated traffic environment and, if desired, influence traffic
dynamics (e.g., a platoon controller adjusting vehicle spacing in response to
a CACC message).

VEINS ships with modules for the full IEEE 1609.x WAVE stack, Cooperative
Awareness Messages (CAMs), Decentralised Environmental Notification Messages
(DENMs), and the V2X application layer defined in ETSI ITS @etsi-its.
This makes it well suited for evaluating standard-compliant V2X applications.
Its limitation for this work is the fidelity gap: VANET routing and security
logic must be reimplemented in C++, so any divergence between the OMNeT++
model and the production Rust implementation is a potential source of false
results. VEINS also provides no means of running arbitrary compiled binaries
inside simulated nodes — only code written against its C++ framework executes.

*VSimRTI* @vsimrti (V2X Simulation Runtime Infrastructure), developed at
the German Aerospace Centre (DLR), takes a federation approach: it defines a
common coupling API that allows multiple simulators to be interconnected — SUMO
for road traffic, ns-3 @ns3 or OMNeT++ for the VANET, and custom application
simulators. VSimRTI acts as the middleware, synchronising simulation time and
routing events between federated components. This enables researchers to swap
out any individual component without restructuring the full simulation setup,
which is valuable for comparative studies. The underlying limitation is the
same as VEINS: application logic must be implemented within the simulation
framework rather than running real executables.

*TraNS* @trans (Traffic and Network Simulation Environment) was an earlier
coupling of ns-2 and SUMO, predating VEINS. It established the now-standard
pattern of bi-directional simulation coupling for VANETs, and while ns-2 has
largely been superseded by ns-3 in new research, TraNS demonstrated that
mobility-driven network simulation was viable and influenced the design of
subsequent coupling frameworks.

*ns-3* @ns3 provides a Wave/80211p module that can be used standalone (with
synthetic mobility models such as the GaussMarkov or RandomWaypoint models) or
coupled with SUMO via TraCI. ns-3's protocol stack includes models of the WAVE
MAC (EDCA, channel coordination, wave service advertisement), the DSRC physical
layer, and IPv6/GeoNetworking. All of this is a model-based reimplementation.

=== Emulation-Based Approaches

Unlike model-based simulators, emulation-based tools run real application code
in an isolated environment, eliminating the fidelity gap at the cost of reduced
scalability and determinism.

*Mininet* @mininet pioneered lightweight network emulation using Linux network
namespaces. Each Mininet host is a namespace with virtual Ethernet links
interconnected by an in-kernel Open vSwitch; latency and bandwidth limits are
applied with kernel traffic control (`tc-netem`). Mininet was designed for
software-defined networking research: its default topology abstraction and
Python API assume OpenFlow-controlled Ethernet switches, and its wireless
support is limited without extensions.

*Mininet-WiFi* @mininet-wifi extends Mininet with IEEE 802.11 wireless
emulation using the `mac80211_hwsim` virtual radio driver in the Linux kernel.
Each node's wireless interface is backed by a `hwsim` radio, which passes
frames through the kernel 802.11 stack. Signal propagation is modelled using
adjustable path-loss models (log-distance, two-ray, ITU) applied in userspace.
Mininet-WiFi is closer to the vehicular use case than plain Mininet, but it is
oriented toward WiFi infrastructure and mesh scenarios; vehicular-speed mobility
and 802.11p WAVE signalling are not first-class concepts.

*GNS3* @gns3 (Graphical Network Simulator-3) is a heavyweight emulation
platform that runs node images in QEMU virtual machines or Docker containers.
Full routing stack binaries (Cisco IOS, Juniper JunOS, FRRouting) execute
natively, giving extremely high execution fidelity for production routers. The
cost is resource overhead: each node requires a separate VM or container with
its full OS image. GNS3 is well suited for realistic networking lab scenarios
but impractical for dense vehicular topologies where tens of nodes must run
concurrently on a single machine.

*Kathara* @kathara replaces GNS3's VM model with Docker containers, reducing
per-node overhead while retaining execution fidelity. A Kathara network is
specified as a directory of container images and interface configuration files,
making topologies reproducible and version-controllable. Kathara has been used
for network function virtualisation and SDN research; its vehicular application
is limited by the same absence of wireless channel modelling present in all
container-based emulators.

*Shadow* @shadow takes a distinct approach: rather than running processes in
separate containers or namespaces, it uses a deterministic discrete-event
execution model. Shadow intercepts system calls (via `preload` or `ptrace`) and
replaces I/O with a simulated network that advances according to a global event
queue. The key benefit is *determinism*: given the same seed, every execution
of a Shadow simulation produces identical results, enabling reproducible network
experiments at scale. Shadow was developed for Tor anonymity network research
and has been used to evaluate Tor at full scale (thousands of relays and
clients) on a single machine. Its limitation for VANET research is that its
network model does not support wireless channels, and the event-driven execution
model makes it difficult to integrate with physical-layer simulation.

*vigilant-parakeet* occupies its own position in this landscape: like
emulation-based tools, it runs real compiled node code (the same `obu_lib`,
`rsu_lib`, and `server_lib` crates that would be deployed on real hardware);
like model-based simulators, it applies a configurable channel model
(latency, loss, jitter) to each link. The channel model is simpler than
VEINS' IEEE 802.11p simulation — no signal attenuation, no collision, no
spatial reuse — but this simplicity is appropriate for studying routing
convergence and security protocols, which depend on delivery statistics
rather than PHY-level behaviour. The core design principle is that any bug
fixed in the production node code is automatically fixed in simulation,
without a separate maintenance burden.

#figure(
  placement: none,
  table(
    columns: (1.5fr, 0.7fr, 1fr, 0.7fr, 1.4fr),
    align: (left, center, center, center, left),
    [*Platform*],            [*Runs real code*], [*Wireless channel*], [*Deterministic*], [*Primary domain*],
    [VEINS/OMNeT++],         [No],  [Yes (802.11p model)], [Yes], [V2X standard compliance],
    [ns-3 + SUMO],           [No],  [Yes (802.11p model)], [Yes], [Protocol stack research],
    [VSimRTI],               [No],  [Yes (federated)],     [Partial], [Multi-simulator coupling],
    [Mininet],               [Yes], [No],                  [No],  [SDN / OpenFlow research],
    [Mininet-WiFi],          [Yes], [Yes (hwsim, 802.11)], [No],  [WiFi / mesh research],
    [GNS3],                  [Yes], [No],                  [No],  [Production router testing],
    [Kathara],               [Yes], [No],                  [No],  [NFV / SDN research],
    [Shadow],                [Yes], [No],                  [Yes], [Large-scale Tor / overlay],
    [Nextmini @nextmini],    [Yes], [No],                  [No],  [Datacenter / ML training],
    [*vigilant-parakeet*],   [*Yes*],[*Configurable*],     [No],  [*VANET routing + security*],
  ),
  caption: [Comparison of network simulation and emulation platforms],
) <tab-sim-comparison>

== Routing Protocol Implementations

Published VANET routing protocol research is predominantly model-based: the
routing algorithm is evaluated in VEINS or ns-3, not in a running Linux
implementation. Production-grade, Linux-native implementations of ad-hoc
routing protocols exist but are generally not vehicular-specific.

*B.A.T.M.A.N.-Advanced* (batman-adv) @batman is a mesh routing protocol
implemented as a Linux kernel module (`net/batman-adv`), operating at Layer 2.
Batman-adv distributes routing responsibility across the network: each node
periodically floods *Originator Messages* (OGMs) carrying a transmission quality
metric derived from observed forwarding success rates. A node selects the
neighbour through which it has historically received the most OGMs from a
given originator as its best transmitter (TQ: Transmission Quality) toward that
originator. Batman-adv is deployed in community mesh networks (Freifunk,
Wireless Battle of the Mesh) and shares vigilant-parakeet's principle of
emitting periodic control beacons for route building, but targets a flat
Layer-2 forwarding model rather than the hierarchical OBU/RSU V2I architecture.

*AODV-UU* @aodv-uu (AODV for Linux, from Uppsala University) is a userspace
daemon implementation of AODV @aodv for Linux. It intercepts packets via
Netfilter hooks, initiates route discovery as needed, and installs kernel
routing table entries for discovered routes. AODV-UU was used for vehicular
research before VEINS-based simulation became dominant. Its limitation in
vehicular contexts — route discovery latency exceeding RSU contact windows —
is the same limitation identified for AODV in @background.

*OLSRd* is the IETF reference implementation of OLSR @olsr, used in community
wireless mesh networks. Like batman-adv, it operates in a relatively stable
pedestrian-speed mesh context and is not designed for vehicular speeds.

The heartbeat-based routing protocol implemented in vigilant-parakeet differs
from all of the above in that it is *infrastructure-anchored*: the RSU is the
routing destination (not an arbitrary peer), and route building is driven by
RSU-emitted beacons rather than node-initiated floods. This model is closer to
the Internet's distance-vector model applied to a single-tier hierarchy (OBUs
routing *toward* a fixed gateway RSU) than to flat mesh routing. No
production Linux implementation of this specific model — heartbeat-driven
upstream candidate selection with N-best caching, composite latency/hop-count
metric, and integrated DH key exchange — was found in the existing literature.

== VANET Security Frameworks

=== Deployed Credential Management Systems

The US *Security Credential Management System* (SCMS) is the operational V2X
PKI infrastructure deployed by the US Department of Transportation and vehicle
manufacturers for connected vehicle programmes. Following the IEEE 1609.2
architecture described in @sec-ieee-1609-2, the SCMS issues and manages
pseudonymous certificates for V2X message signing using ECDSA over P-256.
It separates identity management (Enrolment Authority) from application
authorisation (Policy Certificate Authority) and provides a privacy-preserving
revocation mechanism via Linkage Authorities. The SCMS handles the trust
establishment problem that vigilant-parakeet addresses for OBU–server key
exchange: in vigilant-parakeet, trust is bootstrapped via Ed25519 or ML-DSA-65
signing keys provisioned at manufacture time (PKI mode) or via TOFU on first
contact, rather than a full certificate hierarchy. This simplification is
appropriate for a research prototype but would need to be replaced with a
full credential management system in a production deployment.

The European counterpart is the *C-ITS Credential Management System* (CCMS),
standardised under ETSI ITS @etsi-its, using Authorisation Tickets (ATs) and
the ETSI ITS security header format rather than IEEE 1609.2 WSM headers. Both
systems share the fundamental architecture of short-lived pseudonymous
certificates, hierarchical PKI, and privacy-preserving revocation.

=== Research Prototypes and Security Analysis

@raya-hubaux conducted one of the earliest comprehensive security analyses of
VANETs, identifying the key threats — impersonation, bogus information,
identity disclosure, denial of service — and proposing a certificate-based
security architecture as the solution. Their analysis prefigured much of what
became IEEE 1609.2, and their conclusion that lightweight DH-based session key
establishment is more practical than certificate-based channel encryption under
vehicular connectivity constraints directly motivates the session key
architecture in vigilant-parakeet.

@l3-security-vehicular specifically analyses Layer-3 routing attacks in the
OBU/RSU architecture — the attack model most directly relevant to this work.
That paper identifies routing table poisoning via crafted or replayed heartbeat
messages as the primary control-plane threat, and proposes defences based on
authenticated heartbeats and sequence-number freshness. vigilant-parakeet
implements the replay-freshness defence (the `ReplayWindow` at RSUs) and
provides the infrastructure for studying the remaining authentication
mechanisms identified in that work as future work items.

@vpki presents a comprehensive Vehicular PKI design reconciling privacy,
revocability, and scalability — the design that most influenced the US SCMS
and European CCMS. The pseudonym management challenges described in that
paper (@sec-ieee-1609-2) are relevant to the TOFU vs. PKI design decision in
vigilant-parakeet: TOFU is appropriate in scenarios where PKI infrastructure
is unavailable, but it lacks the revocability properties that make a full PKI
necessary for production deployment.

=== Post-Quantum Security in Vehicular Networks

The post-quantum migration of V2X security is an active research and
standardisation area. @etsi-pqc (ETSI TR 103 619) analyses the quantum threat
to deployed ICT systems, including vehicular communications, and identifies
priority migration targets: key exchange (X25519 → ML-KEM) and digital
signatures (ECDSA/Ed25519 → ML-DSA). The document notes that the vehicular
context presents specific constraints: 802.11p channel access units have a
maximum MPDU size of approximately 2 KB (limited by OFDM symbol count and
channel access duration), which is incompatible with a fully signed ML-KEM-768
key exchange message (~6.5 KB including ML-DSA-65 signature). Proposed
mitigations include message fragmentation, compressed certificate chains, and
hybrid classical+PQC schemes that add only the PQC component (e.g., an
ML-DSA-65 co-signature) to existing ECDSA-signed messages.

@pqc-v2x proposes a lattice-based V2X security scheme (LSSS) derived from
CRYSTALS-Kyber and CRYSTALS-Dilithium (the pre-standardisation precursors to
ML-KEM and ML-DSA). Their evaluation on constrained vehicular hardware
(ARM Cortex-M4) reports key exchange operations taking on the order of tens
of milliseconds — acceptable for session setup but non-trivial on the
low-power embedded units deployed in existing vehicles. The authors also note
the message size challenge: their scheme requires message fragmentation for
802.11p transport.

vigilant-parakeet addresses both concerns differently: it targets a
*software simulator* rather than constrained hardware, so compute latency is
not a first-order constraint; and the simulated transport layer is a TUN
device with configurable MTU (default 1 400 bytes) rather than 802.11p, so
frame size constraints can be relaxed by raising `PACKET_BUFFER_SIZE` to
9 000 bytes. This makes vigilant-parakeet a useful platform for evaluating
post-quantum key exchange overhead in terms of handshake latency and session
establishment time, without the PHY-layer fragmentation problem that complicates
evaluation on 802.11p simulators.

== Summary and Positioning

@tab-related-positioning summarises how vigilant-parakeet compares to the
most relevant related systems across the dimensions most important to this work.

#figure(
  placement: none,
  table(
    columns: (1.5fr, 0.8fr, 0.6fr, 0.6fr, 1.2fr),
    align: (left, center, center, center, center),
    [*System / Work*],
      [*OBU/RSU model*],
      [*Real code*],
      [*PQ crypto*],
      [*Routing security*],
    [VEINS + OMNeT++],     [Yes], [No],  [No],  [Via extensions],
    [ns-3 WAVE],           [Partial], [No], [No], [Research modules],
    [Mininet-WiFi],        [No],  [Yes], [No],  [No],
    [SCMS / CCMS],         [Yes], [N/A], [Planned], [Certificate-based],
    [@raya-hubaux],        [Yes], [No],  [No],  [Analysis only],
    [@l3-security-vehicular], [Yes], [No], [No], [Analysis + design],
    [@pqc-v2x],            [No],  [No],  [Yes], [Signature only],
    [*vigilant-parakeet*], [*Yes*],[*Yes*],[*Yes*],[*Session key + replay window*],
  ),
  caption: [Positioning of vigilant-parakeet relative to related work],
) <tab-related-positioning>

vigilant-parakeet is the only system in this survey that combines: execution
of real compiled VANET node code inside an isolated network namespace; a
configurable post-quantum cipher suite for OBU–server key exchange and
handshake authentication; and a routing security mechanism (HeartbeatReply
replay window) directly addressing the attack model of @l3-security-vehicular.
The trade-offs compared to VEINS are explicit: vigilant-parakeet provides
no 802.11p PHY-layer simulation, no SUMO mobility coupling, and no
standard-compliant V2X application stack. It sacrifices breadth of simulation
coverage to achieve high fidelity for the specific combination of routing
behaviour and cryptographic session establishment that motivates this work.
