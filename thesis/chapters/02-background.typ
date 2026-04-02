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

=== Physical Layer Technologies

Two dominant air-interface standards have been deployed for vehicular
communications:

*DSRC (Dedicated Short-Range Communications)* @dsrc is a licensed 5.9 GHz band
(75 MHz in the US; 30 MHz in Europe under ITS-G5) based on IEEE 802.11p
(WAVE). It provides peer-to-peer broadcast with low latency (typically below
10 ms) and ranges up to approximately 1 km, making it well-suited for safety
applications. Its infrastructure-less operation allows OBUs to communicate
without pre-existing cellular coverage. The standard has been in commercial
deployment in the US, Europe, and Japan since the early 2010s.

*C-V2X (Cellular Vehicle-to-Everything)* @c-v2x encompasses both LTE-V2X
(3GPP Release 14/15) and NR-V2X (Release 16, 5G). C-V2X Mode 4 operates in a
sidelink configuration that does not require network infrastructure coverage,
competing directly with DSRC at the physical layer. 5G NR-V2X adds unicast
and groupcast transmission modes enabling latency-sensitive interactions beyond
what broadcast-only DSRC supports. The two standards are not interoperable at
the physical layer, and their relative merits — reliability under congestion,
non-line-of-sight range, infrastructure cost, standardisation maturity — remain
an active technical and regulatory debate.

The protocols implemented in vigilant-parakeet operate at Layer 3 and above
and are agnostic to the specific air interface: any link delivering packets
between neighbouring nodes with the appropriate latency and loss characteristics
is a valid substrate.

== Routing in Vehicular Networks

A survey of routing approaches in VANETs @vanet-routing-survey identifies
three broad families: topology-based (proactive and reactive), position-based,
and infrastructure-assisted beacon routing. Each presents different trade-offs
under vehicular channel conditions. More recently, delay-tolerant and
cluster-based approaches have been studied for sparse or heterogeneous networks.

=== Topology-Based Proactive Routing

Proactive protocols maintain routing tables continuously via periodic control
exchanges, so that a route is available immediately when a packet must be sent.

*DSDV* (Destination-Sequenced Distance-Vector) @dsdv extends the classical
Bellman-Ford algorithm with sequence-numbered advertisements to eliminate
count-to-infinity loops. Each destination owns a monotonically increasing
sequence number; a node receiving two routes for the same destination keeps the
one with the higher sequence number (more recent), breaking ties by lower cost.
DSDV transmits both periodic full routing-table dumps and event-triggered
incremental updates when a link cost changes. In vehicular environments the
rate of topology change frequently forces full dumps, generating control
overhead that scales as $O(N^2)$ in the number of nodes. Convergence after a
link break requires a new advertisement to propagate across the network, during
which stale routes persist.

*OLSR* (Optimised Link State Routing) @olsr, an IETF standard for MANETs,
reduces link-state flooding overhead through *multipoint relay* (MPR) selection.
Each node selects a minimal subset of its one-hop neighbours that collectively
cover all its two-hop neighbours. Only MPRs retransmit topology control (TC)
messages, pruning the broadcast spanning tree from $O(N^2)$ to significantly
fewer copies. Each node's MPR selector set is reported in TC messages, allowing
every node to reconstruct the global topology and run Dijkstra for shortest-path
computation. OLSR is well-studied for MANETs but problematic for high-speed
VANETs: the MPR set computed for one epoch may be entirely invalid a second
later as vehicles move apart, causing OLSR to route through non-existent links
and generate frequent topology updates that can exceed the capacity of the
control channel.

=== Topology-Based Reactive Routing

Reactive protocols generate routes on demand, reducing steady-state control
overhead at the cost of discovery latency.

*AODV* (Ad-hoc On-Demand Distance Vector) @aodv discovers routes by flooding a
Route Request (RREQ) packet. RREQ propagation is controlled by an expanding
ring search: the TTL field in the IP header starts at a small value and is
doubled on each successive retry if no reply is received, limiting initial
broadcast scope. The destination — or any intermediate node with a fresher
cached route — responds with a Route Reply (RREP) along the reverse path
recorded during RREQ propagation. Active routes are maintained by periodic
Hello beacons between next-hop neighbours; when a link break is detected (Hello
timeout or transmission failure), Route Error (RERR) messages propagate upstream
to invalidate affected routing table entries. AODV maintains per-destination
soft state rather than a global topology map, reducing memory requirements.

*DSR* (Dynamic Source Routing) @dsr eliminates per-hop routing tables entirely
by embedding the complete source-to-destination path in the packet header.
Route discovery is identical in structure to AODV: a Route Request floods the
network, and intermediate nodes append their address to a route-record field.
The source caches the discovered path and uses it for all subsequent packets to
the same destination. DSR benefits from *promiscuous caching*: a node that
overhears a Route Reply passing by caches the discovered path even though it
was not the requester, building up a rich route cache at low control cost.
The cost is larger packet headers proportional to path length and cache
staleness: cached routes are not proactively invalidated and may persist well
past the point where the path is still valid.

In vehicular settings, both AODV and DSR suffer from the route discovery
round-trip exceeding the RSU contact window, making reactive protocols
unsuitable for delay-sensitive applications. AODV's RREQ flooding also produces
the *broadcast storm* problem in dense networks: simultaneous retransmission
by many neighbours causes massive collision at the receiver.

=== Position-Based Routing

Geographic routing exploits GPS coordinates to forward packets toward a
destination without maintaining global topology state. GPSR (Greedy Perimeter
Stateless Routing) @gpsr is the seminal work.

*Greedy forwarding*: each hop selects the neighbour geographically closest to
the destination — i.e., whose Euclidean distance to the destination is smaller
than the forwarding node's own distance. Only one-hop neighbour positions need
be maintained, via periodic position beacons. A packet makes progress toward
the destination at each hop without the sender knowing the full path.

*Perimeter mode*: when no closer neighbour exists (a routing void caused by a
hole in coverage), GPSR switches to planar graph traversal. The network graph
is planarised using either a Gabriel Graph (GG) or Relative Neighbourhood Graph
(RNG) — both remove edges whose midpoints are covered by another node. The
right-hand rule is then applied to traverse the void perimeter: packets always
take the next edge clockwise relative to the arrival direction, guaranteed to
eventually escape the void and resume greedy forwarding. GPSR alternates
between greedy and perimeter modes as packets cross different regions.

*Limitations*: GPSR requires accurate, frequently-updated position information
via position beacons. In vehicular environments, positions change fast enough
that beacons from a second ago may represent an outdated topology. The
planarisation step also assumes symmetric links; VANET channels are frequently
asymmetric, which invalidates the planarity guarantee and can cause GPSR to
loop in perimeter mode. Furthermore, straight-line geometric proximity is a
poor predictor of radio reachability in urban environments where buildings
obstruct paths.

*Road-topology-aware variants*: GPCR (Greedy Perimeter Coordinator Routing)
@gpcr targets urban environments by using road intersections as preferred
forwarders. Rather than pure Euclidean proximity, the routing metric favours
nodes that lie on the road segment leading toward the destination. Perimeter
traversal follows road graph edges rather than a planar graph extracted from
unreliable position data. A-STAR @astar augments this with an attractiveness
metric derived from bus-route maps: packets prefer roads with higher expected
vehicle density, improving delivery ratio in sparse urban networks by biasing
toward well-trafficked corridors where relay opportunities are frequent.

=== Delay-Tolerant Routing

When end-to-end connectivity cannot be guaranteed — sparse rural VANETs,
infrastructure gaps, or highly intermittent channels — *store-carry-forward*
(Delay-Tolerant Network, DTN) approaches relax the requirement for a path to
exist at the time of transmission @vadd. A vehicle carries a message in its
buffer as it drives, and delivers when it comes within range of an appropriate
relay or the final destination.

VADD (Vehicle-Assisted Data Delivery) @vadd models each road intersection as a
choice point. At each intersection, the forwarding vehicle selects the adjacent
road segment with minimum *expected packet delivery delay*, computed from
average vehicle speed, traffic density, and segment length. This is equivalent
to shortest-path computation on a mobility-weighted road graph rather than an
instantaneous connectivity graph. VADD's key insight is that road-level
mobility statistics — accessible from traffic databases or historical traces —
are more stable and predictable than instantaneous V2V topology. Its limitation
is dependence on accurate speed models; sudden traffic events (accidents, road
closures) can invalidate predictions and cause packets to be carried on
suboptimal trajectories.

=== Cluster-Based Routing

In dense networks, flat flooding of control messages is inefficient. Cluster-
based approaches @cluster-routing organise nodes into groups with a *cluster
head* (CH) aggregating routing state and communicating with other CHs via an
inter-cluster backbone. Member nodes route through their CH, reducing the number
of nodes participating in global route computation.

The central challenge in vehicular clustering is stability: high relative
mobility causes frequent CH elections and member migration. Stability metrics
such as relative velocity between member and CH candidate, transmission range
overlap, and connection duration are used to select CHS likely to remain in
range @cluster-routing. Despite this, cluster lifetime in freeway scenarios is
typically tens of seconds — far shorter than in static MANETs — limiting the
benefit of cluster-level state aggregation. Cluster-based routing is most
effective in relatively stable scenarios such as bus fleets or convoy
formations with predictable mobility.

=== Beacon / Heartbeat Routing

Infrastructure-assisted vehicular networks use periodic *heartbeat* or
*beacon* messages emitted by RSUs to drive route selection. OBUs overhearing
heartbeats measure propagation delay, record hop count, and build upstream
routing tables toward the emitting RSU. This approach suits V2I settings where
connectivity to a fixed RSU is the primary goal: the routing problem reduces
to selecting the best relay path toward a known, stationary gateway.

The metric for path selection varies across implementations. Hop-count alone
is simple but insensitive to per-link quality differences; RTT-based metrics
better reflect actual channel conditions but require reply messages and may
exhibit oscillation under asymmetric loss. Hybrid metrics — combining a
latency component with a hop-count fallback — are common in practice and are
the basis of the routing protocol implemented in this work
(see @sec-routing-protocol).

=== Comparative Analysis

#figure(
  table(
    columns: (1fr, 1fr, 1fr, 1fr, 1fr),
    align: (left, left, left, left, left),
    [*Protocol family*], [*State maintained*], [*Control overhead*],
      [*Route latency*], [*VANET suitability*],
    [Proactive\ (DSDV, OLSR)],
      [Global topology],
      [High — continuous $O(N^2)$ updates],
      [Zero],
      [Poor — overhead scales with density; high churn],
    [Reactive\ (AODV, DSR)],
      [Per-route soft state],
      [Low (on demand)],
      [High — discovery flood may outlast RSU contact window],
      [Poor — broadcast storm; discovery latency incompatible with vehicular contacts],
    [Position-based\ (GPSR, GPCR)],
      [Neighbour positions only],
      [Low — position beacons],
      [Zero],
      [Moderate — void problem; urban adaptation needed; GPS accuracy required],
    [DTN\ (VADD)],
      [Local buffer + road map],
      [Minimal],
      [Unbounded — carry-forward],
      [Good for sparse/intermittent networks; unsuitable for low-latency applications],
    [Cluster-based],
      [Cluster topology],
      [Moderate — CH elections],
      [Low within cluster],
      [Limited — short cluster lifetime in fast vehicular networks],
    [Beacon/heartbeat\ *(this work)*],
      [Per-RSU upstream tables],
      [Low — RSU-driven periodic beacons],
      [Zero for established routes],
      [Good for V2I: RSU reachability is proactive; metric sensitive to link quality],
  ),
  caption: [Summary comparison of VANET routing families],
) <tab-routing-comparison>

== Security in Vehicular Networks

=== Attack Taxonomy

Vehicular networks are exposed to a range of adversarial behaviours spanning
both the data plane (payload forwarding) and the control plane (routing). The
following taxonomy covers the main attack classes studied in the literature
and addressed, in whole or in part, by the security architecture implemented
in this work.

- *Routing table poisoning*: an adversary controlling one or more intermediate
  nodes selectively drops, replays, or crafts heartbeat messages to corrupt
  routing state across a wide area, attracting traffic through
  attacker-controlled paths. @l3-security-vehicular provides a detailed
  analysis of this class of attack in the OBU/RSU architecture. Defences
  require authenticated heartbeats (see @sec-broadcast-auth) and sequence-number
  freshness checks.

- *Black hole and grey hole attacks*: a node advertises an artificially
  attractive route then silently drops (black hole) or probabilistically
  forwards (grey hole) the attracted traffic. Detection relies on
  neighbourhood monitoring: a watchdog node in promiscuous mode can confirm
  whether its next hop forwarded the packet it received @watchdog. The
  CONFIDANT protocol @confidant extends watchdog monitoring with a distributed
  reputation system in which nodes share misbehaviour observations and route
  around nodes with poor accumulated reputation.

- *Sybil attacks* @sybil: a single physical node presents multiple false
  MAC or IP identities, polluting routing tables and consensus mechanisms
  with phantom participants. Sybil identities are particularly harmful in
  heartbeat-based routing because each fake identity can independently
  attract traffic and manipulate route scores. Certificate-based identity
  binding (IEEE 1609.2, @sec-ieee-1609-2) mitigates Sybil attacks under the
  assumption that a PKI cannot issue unbounded certificates to a single physical
  node.

- *Wormhole attacks* @wormhole: two colluding nodes tunnel packets between
  distant network segments out-of-band, creating the illusion of a
  low-latency shortcut. Neighbours at both tunnel endpoints observe apparently
  direct connectivity, preferring the wormhole path; the colluding pair can
  then selectively manipulate forwarded traffic. Detection approaches include
  packet leashes @wormhole — geographic leashes embed GPS position and
  timestamp; temporal leashes bound transmission delay using synchronised clocks
  — and directional-antenna methods, but both require additional hardware or
  tight clock synchronisation.

- *Replay attacks*: previously captured control messages (e.g., heartbeats
  with high sequence numbers) are re-injected to maintain stale routing
  entries or to exhaust sequence counters. Monotone sequence numbers combined
  with short validity windows (one heartbeat interval) bound replay exposure.

- *Passive eavesdropping*: a physically present adversary captures payload
  frames from any link, recovering application data from unencrypted
  transmissions. Addressed by end-to-end AEAD encryption with session keys
  never held by relay nodes (see @security).

- *Position spoofing*: a node broadcasts false GPS coordinates to manipulate
  position-based routing decisions. Cross-checking claimed positions using
  time-of-arrival measurements from multiple anchor RSUs can detect inconsistencies
  indicating fabricated positions; this remains an open problem for single-RSU
  scenarios.

=== PKI-Based Security: IEEE 1609.2 and ETSI ITS <sec-ieee-1609-2>

IEEE 1609.2 @ieee-1609-2 defines the security architecture for Wireless Access
in Vehicular Environments (WAVE). The architecture is a tiered PKI consisting
of:

- *Root CA*: the trust anchor. Its certificate is embedded in vehicle firmware
  at manufacture time. It signs the certificates of Intermediate CAs and is
  kept offline to limit exposure.

- *Intermediate CAs / Policy CAs*: issue certificates to lower-level
  authorities with scoped permissions expressed as *service-specific
  permissions* (SSP) bit strings. A certificate for a given ITS application
  (safety messaging, intersection management, etc.) carries only the SSP for
  that application.

- *Pseudonymous Certificate Authority (PCA)*: issues short-lived
  *pseudonymous* end-entity certificates to vehicles. A PCA knows only that a
  device holds a valid enrolment credential, not its real-world identity.

- *Enrolment CA*: issues a long-lived *enrolment credential* to a vehicle at
  manufacture time. The enrolment credential is used only to request
  pseudonymous certificates and is never transmitted in safety messages,
  preventing correlation by passive observers.

- *Linkage Authority (LA) and Misbehavior Authority (MA)*: provide a
  privacy-preserving revocation infrastructure. Each certificate carries a
  *linkage value* (LV) — a compact identifier derivable from a secret held by
  the LA. If a vehicle is reported for misbehaviour, the MA coordinates with
  the LA to generate CRL entries based on the LV without revealing the
  vehicle's real identity to any single entity (separation of knowledge).

The core operational mechanisms are:

- *Signed messages*: every safety-critical Wireless Short Message (WSM) is
  signed with ECDSA over P-256 or P-384, binding it to a pseudonymous
  certificate. The signature covers the message payload and a timestamp,
  preventing both tampering and replay.

- *Pseudonymous certificates*: vehicles hold a pool of short-lived certificates
  (validity period configurable; the US SCMS model provisions batches of
  approximately 1-week certificates). Vehicles rotate to a new pseudonym at
  privacy-sensitive moments (at intersections or after a fixed interval of
  use) to prevent long-term vehicle tracking via certificate continuity.

- *Certificate revocation*: compromised certificates are invalidated via
  CRL distribution. Distributing CRLs under intermittent VANET connectivity
  is an open engineering problem: proposed solutions include RSU-based CRL
  push at roadside, Bloom-filter-compressed CRL entries embedded in beacon
  messages, and V2V epidemic broadcast of revocation notices.

The ETSI ITS security framework @etsi-its complements IEEE 1609.2 with
Authorisation Ticket (AT) mechanisms for Europe. An AT is a short-lived
certificate authorising a specific ITS application permission. The ETSI model
separates registration from authorisation: the Enrolment Authority (EA) and
Authorisation Authority (AA) are distinct entities, so an AA cannot link an
AT back to a vehicle's real identity, only confirm the vehicle holds a valid
EA credential.

A comprehensive design for a Vehicular PKI (VPKI) reconciling privacy,
revocability, and operational scalability is presented by Papadimitratos
et al. @vpki. The design influenced both the US Security Credential Management
System (SCMS) and the European C-ITS credential management system.

=== Pseudonym Management

Pseudonymous certificates protect long-term privacy against passive observers
correlating a vehicle across its journey via certificate identity. Effective
pseudonym management requires balancing several competing concerns:

- *Pool size and provisioning*: a vehicle should hold enough certificates to
  rotate frequently without exhausting its supply between provisioning windows.
  The SCMS model provisions batches of certificates over the cellular network
  during periodic software updates.

- *Change frequency and timing*: fixed-interval rotation is weaker than random
  rotation, as an adversary correlating by timing can still link successive
  certificates. Changing pseudonym at busy intersections — where many vehicles
  change simultaneously, providing a crowd for $k$-anonymity — is more
  effective than changing on an empty road where a single vehicle changes
  and the change is trivially attributable.

- *Cross-layer linkability*: even with certificate rotation, higher-layer
  identifiers (IP addresses, application session tokens, Bluetooth beacons)
  can re-link a vehicle to its previous pseudonym. Full privacy requires
  coordinated re-randomisation of all identifiers simultaneously with the
  certificate change.

- *Linkability vs. revocability trade-off*: shorter certificate lifetimes and
  larger pools make tracking harder, but also make it harder for the MA to
  revoke all certificates belonging to a misbehaving vehicle before it rotates
  to new, unrevoked certificates.

=== Broadcast Authentication <sec-broadcast-auth>

Heartbeat and beacon messages are inherently broadcast, which complicates
authentication. A receiver cannot know in advance the sender's public key to
verify an asymmetric signature, and distributing a pre-shared group key for
HMAC creates difficult key revocation challenges.

*Asymmetric signatures* (ECDSA as in IEEE 1609.2) provide the strongest
guarantees: a receiver that obtains the sender's certificate can verify
immediately and non-interactively. However, verification is computationally
expensive relative to a heartbeat period. On embedded hardware,
ECDSA-P256 verification takes on the order of 1 ms; at vehicular densities of
hundreds of beacons per second from many neighbours, this load becomes
significant. Certificate transmission overhead (a P-256 certificate is
approximately 250 bytes) also inflates beacon size substantially.

*TESLA* (Timed Efficient Stream Loss-tolerant Authentication) @tesla uses a
*time-delayed key disclosure* mechanism to amortise the cost of asymmetric
bootstrapping over many messages:

+ At bootstrap, the sender constructs a one-way hash chain of length $n$:
  starting from a random seed $k_n$, it computes
  $k_(n-1) = H(k_n), k_(n-2) = H(k_(n-1)), dots.c, k_0 = H(k_1)$.
  The sender commits to $k_0$ (the chain head) via a single asymmetrically
  signed bootstrapping message distributed to receivers.

+ To broadcast a message $m_i$ in time epoch $i$, the sender uses $k_i$ as a
  MAC key: $"tag"_i = "HMAC"(k_i, m_i)$. At the time of
  broadcast, $k_i$ has not yet been disclosed to receivers.

+ In a subsequent epoch $j > i$, the sender broadcasts $k_i$. Receivers verify
  that $H^(j-i)(k_i) = k_(j-i)$ (confirming the key is on the pre-committed
  chain), then retroactively verify $"tag"_i$ on buffered message $m_i$.

Security relies on receivers having an upper bound on sender–receiver clock
skew: a receiver must be certain that $k_i$ was not already known (i.e.,
already disclosed) at the time $m_i$ was received. In vehicular networks, GPS
provides a readily available, high-accuracy time reference satisfying this
requirement. TESLA introduces an authentication latency of one disclosure
interval (one heartbeat period typically), which must be weighed against the
routing-stability benefit. An attacker who can inject packets in the disclosure
window before verification can still cause temporary routing disruption, but
cannot forge persistent authenticated messages.

*HMAC* @rfc2104 provides integrity and authenticity on unicast paths where a
shared symmetric key has been established in advance. HMAC is appropriate for
the OBU–server unicast channel once a DH session key is in place, but cannot
be used for broadcast heartbeats where no pre-shared key exists between
sender and arbitrary receivers.

=== Misbehavior Detection and Revocation

Certificate-based authentication prevents impersonation of honest nodes but
cannot prevent a legitimately-credentialed node from behaving maliciously:
dropping packets, injecting false position or routing information, or
duplicating its identity across multiple physically distinct devices.

*Watchdog monitoring* @watchdog places nodes in promiscuous mode to overhear
whether their next-hop relay actually forwards packets. A watchdog increments a
failure counter each time it transmits a packet and does not observe the next
hop forwarding it; above a threshold the next hop is removed from routing
candidates. Limitations include collusion (two nodes confirm forwarding between
each other without actually forwarding to the destination) and full-duplex
separation (the next hop may forward on a channel the watchdog cannot observe).

*CONFIDANT* (Cooperation of Nodes — Fairness in Dynamic Ad-hoc NeTworks)
@confidant extends watchdog monitoring with a reputation system. Nodes share
ALARM messages reporting misbehaviour observations to a trust manager.
The reputation score per neighbour is updated on each ALARM; nodes with a
score below a threshold are excluded from routing. CONFIDANT is vulnerable to
*slander attacks* where a malicious coalition fabricates ALARMs against honest
nodes; weighting ALARMs by the reputation of their source partially mitigates
this at the cost of a bootstrapping problem (new nodes have unknown reputations).

*Certificate revocation* closes the loop at the PKI level: the MA collects
misbehavior evidence from the field, adjudicates, and instructs the LA to
generate CRL entries for the offending vehicle's current and future
pseudonymous certificates. A persistent misbehaver can thus be permanently
excluded from the network even as it rotates pseudonyms. The challenge is
distributing revocation information quickly enough that the window between
detection and effective exclusion does not allow significant damage.

=== Cryptography Primer

A brief summary of the cryptographic primitives used in this project:

- *X25519 (Curve25519 Diffie–Hellman)* @x25519: an elliptic-curve DH key
  exchange over the Montgomery curve Curve25519. Each party generates a
  32-byte scalar private key $a$ and computes the public key $A = a dot G$,
  where $G$ is the curve base point. Both parties compute the shared secret
  $S = a dot B = b dot A$. The scalar multiplication is constant-time and
  provides 128-bit security.

- *HKDF (HMAC-based Key Derivation Function)* @hkdf: the two-step
  Extract-then-Expand construction. HKDF-Extract(salt, IKM) = HMAC(salt, IKM)
  produces a uniform pseudorandom key (PRK); HKDF-Expand(PRK, info, L) uses
  repeated HMAC applications to produce $L$ output bytes, domain-separated by
  the `info` string. This ensures that the raw shared secret from X25519 is
  stretched and domain-separated into symmetric keys of the required length.

- *AEAD (Authenticated Encryption with Associated Data)*: AEAD constructions
  (AES-GCM, ChaCha20-Poly1305) provide both confidentiality and integrity.
  Encryption outputs ciphertext plus a 16-byte authentication tag computed over
  both the ciphertext and optional associated data; decryption rejects the
  plaintext if the tag does not verify. Nonce uniqueness is required: this
  implementation generates a 12-byte nonce from the OS CSPRNG for each
  encryption.

- *Ed25519 signatures* @rfc8032: an EdDSA scheme over Edwards25519. A
  signing key produces a deterministic 64-byte signature over a message;
  verification uses the corresponding verifying (public) key. Ed25519 is
  used optionally in this work to authenticate DH handshake messages, binding
  them to node identities and preventing active man-in-the-middle substitution.

These primitives compose as follows: X25519 produces a shared secret; HKDF
derives symmetric keys; AEAD encrypts application payloads; Ed25519 (when
enabled) signs handshake messages to authenticate the key exchange endpoints.

=== Data Confidentiality

Early vehicular network designs prioritised availability and integrity — safety
messages must arrive and must not be modified — treating payload confidentiality
as secondary. As VANETs expand to carry non-safety traffic (internet access,
toll transactions, content delivery), protecting payload data from relay nodes
and eavesdroppers becomes significant.

The standard approach is *end-to-end encryption* at the OBU–server boundary:
intermediate relay nodes (other OBUs, RSUs) forward ciphertext opaquely and
never hold session keys. The central challenge in the VANET context is
completing a key exchange within the contact window an OBU may have with a
given RSU. Two-message Diffie-Hellman handshakes @x25519 are lightweight
enough to complete in a single RSU contact and produce a forward-secret session
key without requiring certificate infrastructure. Raya and Hubaux @raya-hubaux
analyse the broader trade-offs between pseudonymous certificate-based approaches
and lightweight alternatives for vehicular security, concluding that lightweight
DH-based schemes are more practical under the connectivity constraints of
vehicular networks.

=== The Reference Security Model

The paper by @l3-security-vehicular specifically analyses Layer-3 security
threats in vehicular networks, with a focus on routing manipulation attacks
in the OBU/RSU architecture. A central finding is that an adversary controlling
one or more intermediate nodes can selectively drop, replay, or modify heartbeat
messages to poison routing tables across a wide area — an attack that is
efficient precisely because heartbeats carry no authentication. The routing
protocol implemented in vigilant-parakeet is designed to reproduce the network
model described in that work and serve as a platform for studying both the
attacks and the defences proposed therein.

== Simulation Approaches

=== Network Simulators

Traditional network simulators — ns-3 @ns3, OMNET++ @omnetpp, and SUMO
@sumo (for mobility) — provide rich, well-validated models but require
significant configuration effort, are difficult to extend with custom Rust
code, and do not run actual production-grade node logic.

*ns-3* is a discrete-event network simulator implemented in C++ with a Python
scripting API. It provides detailed models of 802.11p (WAVE), IEEE 1609.x,
LTE, and many other protocols. Vehicular scenarios are typically coupled with
SUMO for realistic mobility traces via the TraCI interface. ns-3 simulates a
synthetic reimplementation of the protocol stack on a single simulated host,
not the actual binary that would be deployed on hardware.

*OMNeT++ with VEINS* provides an analogous SUMO coupling for vehicular
simulations. The VEINS framework implements the IEEE 802.11p PHY/MAC and the
IEEE 1609.x WAVE stack in C++ within OMNeT++'s module system. Like ns-3,
OMNeT++/VEINS is a model-based simulator: protocol behaviour is reimplemented
from specification rather than running real node code.

The fundamental limitation of model-based simulators for this work is the
*fidelity gap*: any difference between the C++ simulation model and the Rust
production code is a potential source of incorrect simulation results. Bugs in
the simulator do not manifest on hardware, and bugs fixed on hardware do not
automatically flow into the simulator.

=== Container and Namespace-Based Simulation

Linux *network namespaces* provide lightweight OS-level isolation of the
network stack. Each namespace has its own routing table, interface list, and
firewall rules, but shares the host kernel. This allows the same node binary
that would run on real hardware to execute in an isolated environment with
controlled inter-namespace connectivity, eliminating the fidelity gap.

Tools such as Mininet @mininet popularised this approach for software-defined
networking research. vigilant-parakeet applies the same technique to vehicular
nodes: each OBU, RSU, and server executes in its own namespace with TUN/TAP
virtual interfaces providing L2 connectivity. A userspace channel model applies
configurable latency, packet loss, and jitter to inter-node links without
requiring kernel `tc-netem` rules — all link emulation is handled in Rust
async tasks, making the simulation fully portable within the Linux kernel.

A closely related contemporary work is *Nextmini* @nextmini, a general-purpose
network emulation testbed targeting datacenter and cloud networking research.
Like vigilant-parakeet, Nextmini is implemented in Rust with the Tokio async
runtime, uses Linux network namespaces for node isolation, and exposes TUN
interfaces so that arbitrary application binaries can run inside the emulated
network without modification. Its dataplane adopts an *actor model* in which
each node's forwarding logic runs as one or more Tokio tasks communicating via
MPSC channels, avoiding shared mutable state across the dataplane.

Nextmini's architecture diverges from vigilant-parakeet in several respects
that reflect its different target domain:

- *Control plane*: Nextmini separates control from data through a centralised
  Rust web server controller backed by a PostgreSQL database. Control-plane
  algorithms are expressed as Python scripts that query and update the database;
  real-time PostgreSQL triggers notify the controller when routing decisions
  must be pushed to the dataplane. vigilant-parakeet's routing is fully
  distributed — RSUs emit heartbeats, OBUs build routing tables independently
  with no central coordinator.

- *Inter-node connectivity*: Nextmini connects dataplane nodes via persistent
  TCP or QUIC connections, optimised for high bulk throughput (up to 132 Gbps
  in its `max` mode using the Linux `splice` system call). vigilant-parakeet
  uses TUN/TAP virtual interfaces with a userspace channel layer that imposes
  per-link latency, loss, and jitter — a model suited to studying routing
  under impaired wireless conditions rather than maximising throughput.

- *Scale and target*: Nextmini targets up to 10,000 dataplane nodes on a
  single machine (approximately 4.5 MB per node) and is designed for
  large-scale datacenter experiments including distributed ML training.
  vigilant-parakeet targets small-to-medium vehicular topologies (tens of
  nodes) where the focus is routing protocol correctness and security rather
  than forwarding throughput.

- *Channel impairment model*: Nextmini does not model wireless channel
  conditions — latency and loss are not first-class concepts in its topology
  description. vigilant-parakeet's per-link `Channel` objects with configurable
  latency, loss, and jitter are central to its purpose of evaluating routing
  behaviour under vehicular channel conditions.

- *Domain specificity*: Nextmini is domain-agnostic: it runs any application
  workload (web servers, ML training jobs) and any routing algorithm expressible
  in Python. vigilant-parakeet encodes VANET-specific behaviour (OBU/RSU roles,
  heartbeat-based route building, DH key exchange, N-best candidate caching)
  directly in its node library crates.

In summary, Nextmini and vigilant-parakeet occupy different positions on the
spectrum between general-purpose emulation infrastructure and domain-specific
simulation: Nextmini provides a highly scalable, flexible substrate for
datacenter networking research with no built-in protocol assumptions;
vigilant-parakeet sacrifices generality to provide first-class support for
vehicular routing dynamics, wireless channel impairment, and the OBU–RSU
security architecture that motivates this work.

The key advantage that both share over model-based simulators is *fidelity*:
the same binary runs in simulation and in production. Bugs manifest in both
environments; fixes in production automatically flow into the simulator.

=== Rust for Systems Programming

Rust @rust provides memory safety without garbage collection, making it
well-suited for implementing network protocol logic that must be both correct
and efficient. The Tokio @tokio asynchronous runtime allows many nodes to run
concurrently on a shared thread pool, enabling dense simulations (dozens of
nodes) on commodity hardware without per-node OS threads. The async model
also makes latency injection natural: a sleeping `tokio::time::sleep` future
correctly models a link delay without blocking any OS thread.
