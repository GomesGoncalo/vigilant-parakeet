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

== V2X Application Domains and QoS Requirements

The design of a VANET routing and security layer cannot be evaluated in
isolation from the applications it carries. Each V2X application class places
qualitatively different demands on the network in terms of latency, reliability,
and message rate, which in turn determine the feasibility of various routing
and security mechanisms. @safetyapps surveys the major application categories.

=== Safety-Critical Applications

Safety applications are the primary driver of the DSRC/IEEE 802.11p standard
and represent the most demanding QoS class. They include:

/ *Cooperative Collision Avoidance (CCA)*: OBUs broadcast position, speed,
  and heading at 10 Hz. Receiving vehicles detect trajectories that will
  intersect within a collision horizon and warn the driver or apply automatic
  braking. ETSI ITS @etsi-its-latency specifies an end-to-end latency budget of
  100 ms for safety-critical alerts, with the radio access layer alone capped
  at 10–50 ms.

/ *Intersection Management*: RSUs broadcast signal phase and timing (SPAT)
  and map data (MAP) messages to approaching vehicles. Latency of tens of
  milliseconds allows OBUs to optimise speed to catch a green phase (green
  wave) or to brake before a red one.

/ *Emergency Vehicle Notification*: police, ambulance, and fire vehicles
  broadcast priority alerts. Relay OBUs forward the alert on DSRC;
  infrastructure RSUs relay to out-of-range areas. Latency requirements are
  below 500 ms over a 300 m radius.

/ *Post-Crash Notification*: a crashed vehicle broadcasts an eCall trigger
  to nearby OBUs, which relay it to an RSU for forwarding to emergency services.
  The V2X path serves as a backup when the in-vehicle cellular modem is
  inoperative or blocked by metalwork.

All safety applications share the properties of: small message payloads
(CAM messages are typically 200–400 bytes); broadcast or geocast delivery
(authenticated but not encrypted under IEEE 1609.2, since the payload is meant
to be received by any nearby vehicle); and strict latency requirements
(typically 10–100 ms) that rule out reactive route discovery protocols.

=== Traffic Efficiency Applications

Traffic efficiency applications operate on a seconds-to-minutes time scale and
can tolerate somewhat higher latency than safety applications in exchange for
coordinated vehicle behaviour:

/ *Cooperative Adaptive Cruise Control (CACC)*: vehicles in a platoon
  exchange acceleration and braking intentions at 10–50 Hz. CACC allows
  inter-vehicle gaps as short as 0.3–1 s at highway speeds by providing advance
  notice of upstream deceleration, allowing following vehicles to brake before
  the deceleration propagates backward through the platoon. Latency requirements
  are 10–20 ms for effective gap closing @platooning.

/ *Truck Platooning*: multiple trucks drive in close convoy under joint
  electronic control. The lead truck broadcasts velocity commands; followers
  adjust throttle and brakes via V2V. Since the platoon can span several hundred
  metres, multi-hop relaying with controlled latency is required. End-to-end
  latency must be bounded: unbounded latency variance causes platoon instability
  and potential collision. This is precisely the use case for the N-best
  upstream caching and latency-based routing metric in vigilant-parakeet.

/ *Traffic Signal Phase Optimisation*: RSUs can receive aggregate traffic
  density information from approaching OBUs and adapt signal timing accordingly.
  Unicast or groupcast delivery via V2I with latency on the order of one signal
  phase (tens of seconds) is sufficient.

=== Infotainment and Value-Added Services

Non-safety applications carry internet traffic, parking availability data,
map updates, and digital content downloads from infrastructure RSUs. These are
typically unicast TCP sessions requiring reliable delivery but tolerating
latency on the order of hundreds of milliseconds to seconds. The OBU acts as
a mobile gateway, and the key routing challenge is hand-off: maintaining session
continuity as the vehicle moves between RSU coverage areas. The DH session key
architecture in vigilant-parakeet is designed for this use case: the session
key is between the OBU and a backend server, not the OBU and the current RSU,
so RSU hand-off does not require rekeying.

=== Cooperative Perception for Automated Driving

Emerging autonomous driving applications share raw sensor data — camera images,
LiDAR point clouds, radar returns — between nearby vehicles and roadside sensors
to extend beyond each vehicle's direct sensor horizon @coop-perception. This
data sharing (Collective Perception Messages, CPMs) demands very high throughput
(potentially tens of Mbps per vehicle) and low latency (50–100 ms for useful
fusion). These requirements push beyond what 802.11p can deliver at vehicular
densities, motivating 5G NR-V2X sidelink for this application class. The routing
and security protocols studied in vigilant-parakeet are relevant to the control
plane of such systems (session establishment, relay selection) even if the data
plane would require different transport.

=== QoS Implications for Routing and Security

#figure(
  table(
    columns: (1fr, auto, auto, auto, auto),
    align: (left, center, center, center, center),
    [*Application class*], [*Latency budget*], [*Reliability*], [*Payload*], [*Encrypted?*],
    [Safety (CAM, DENM)],    [$<$100 ms],  [99.999%], [200–400 B], [No (signed)],
    [CACC / platooning],     [$<$20 ms],   [99.99%],  [100–200 B], [Optional],
    [Traffic efficiency],    [$<$1 s],     [99%],     [0.5–2 KB],  [Optional],
    [Infotainment / data],   [$<$5 s],     [95%],     [large],     [Yes],
    [Cooperative perception],[50–100 ms],  [99.9%],   [MB-range],  [Yes],
  ),
  caption: [QoS requirements across V2X application classes],
) <tab-qos>

The table illustrates why the routing metric matters: for CACC and platooning,
route oscillation of even a few tens of milliseconds per interval is
disruptive. The 10% hysteresis threshold in vigilant-parakeet's route selection
(see @sec-routing-protocol) is calibrated to prevent exactly this oscillation.
For safety applications, the absence of payload encryption is a deliberate
design choice in the standards: CAM messages are public safety information, and
encryption would prevent roadside sensors and other non-paired receivers from
using the broadcast. The data-plane encryption in vigilant-parakeet applies to
the infotainment-class unicast sessions that require confidentiality.

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

- *ML-KEM-768 (Module Lattice Key Encapsulation Mechanism)* @fips203: a
  quantum-resistant Key Encapsulation Mechanism standardised as NIST FIPS 203.
  Unlike DH, it does not require both parties to contribute randomness to the
  shared secret; instead, one party (the encapsulator) generates the secret and
  transmits it encrypted under the other's public encapsulation key. The
  encapsulation key is 1184 bytes, the ciphertext 1088 bytes, and the shared
  secret 32 bytes. Its security rests on the hardness of the Module Learning
  With Errors (MLWE) problem rather than on discrete logarithms, making it
  resistant to Shor's algorithm @shor94 (see @sec-pqc).

- *ML-DSA-65 (Module Lattice Digital Signature Algorithm)* @fips204: a
  quantum-resistant signature scheme standardised as NIST FIPS 204, used as a
  drop-in replacement for Ed25519 when quantum resistance is required. The
  verifying key is 1952 bytes and the signature 3309 bytes. Like ML-KEM-768 it
  is based on MLWE and is not vulnerable to known quantum attacks (see @sec-pqc).

These primitives compose as follows: X25519 or ML-KEM-768 produces a shared
secret; HKDF derives symmetric keys; AEAD encrypts application payloads;
Ed25519 or ML-DSA-65 (when enabled) signs handshake messages to authenticate
the key exchange endpoints. The classical pair (X25519 + Ed25519) and the
quantum-resistant pair (ML-KEM-768 + ML-DSA-65) are independently configurable.

=== Post-Quantum Cryptography <sec-pqc>

==== The Quantum Threat

Shor @shor94 demonstrated in 1994 that a sufficiently large quantum computer
can solve the integer factorisation and discrete logarithm problems in
polynomial time. This breaks all widely deployed asymmetric cryptography whose
security relies on those problems: RSA, Diffie-Hellman over finite fields, and
all elliptic-curve variants including X25519 (ECDH) and Ed25519 (ECDSA/EdDSA).

The practical timeline for cryptographically relevant quantum computers remains
uncertain, but the *harvest now, decrypt later* (HNDL) attack is a present
threat: an adversary with sufficient storage can capture encrypted traffic
today and decrypt it retroactively once quantum hardware matures. For long-lived
infrastructure such as vehicular network nodes — which may remain in service for
a decade or more — the HNDL window is a genuine concern, motivating
quantum-resistant key exchange now rather than at the point of quantum
availability. Grover's algorithm provides a quadratic speedup for brute-force
search, halving the effective security of symmetric ciphers; AES-256 retains
128-bit post-quantum security and is unaffected by this work.

==== NIST Post-Quantum Cryptography Standardisation

In 2016 NIST launched an open competition to standardise post-quantum
cryptographic algorithms. After three evaluation rounds involving the global
cryptographic community, NIST selected and standardised four algorithms in
2024:

- *FIPS 203 — ML-KEM* @fips203: key encapsulation (based on CRYSTALS-Kyber)
- *FIPS 204 — ML-DSA* @fips204: digital signatures (based on CRYSTALS-Dilithium)
- *FIPS 205 — SLH-DSA*: stateless hash-based signatures (based on SPHINCS+)
- *FIPS 206 — FN-DSA*: fast lattice-based signatures (based on FALCON)

vigilant-parakeet implements ML-KEM-768 (FIPS 203) and ML-DSA-65 (FIPS 204),
the NIST Security Level 3 parameter sets of the two primary lattice-based
standards. NIST Security Level 3 targets security equivalent to AES-192,
meaning no known classical or quantum algorithm can break the scheme faster
than exhaustive search of a 192-bit key space.

==== Module Learning With Errors

Both ML-KEM and ML-DSA derive their security from the *Module Learning With
Errors* (MLWE) problem, a structured variant of the Learning With Errors (LWE)
problem introduced by Regev @lwe. Given a polynomial ring $R_q = ZZ_q [x] \/ (x^n + 1)$
with $n = 256$ and a prime modulus $q$, the MLWE problem is:

*Given* a random matrix $bold(A) in R_q^(k times k)$, a secret vector
$bold(s) in R_q^k$ with small coefficients, and an error vector
$bold(e) in R_q^k$ with small coefficients, *distinguish*
$bold(A), bold(A) bold(s) + bold(e)$ from $bold(A), bold(u)$ where
$bold(u)$ is uniformly random.

For ML-KEM-768 and ML-DSA-65, $k = 3$ (the "768" designates the total ring
dimension $k dot n = 768$). No polynomial-time classical or quantum algorithm
is known for MLWE; the best known algorithms require sub-exponential time even
on quantum hardware. The reduction from MLWE to ring-LWE and standard LWE is
well-studied, providing a firm theoretical foundation for the security claims.

==== ML-KEM-768: Key Encapsulation Mechanism

A KEM differs from Diffie-Hellman in a fundamental way: it is *asymmetric in
the production of the shared secret*. The holder of the encapsulation key
(analogous to a public key) can only receive a secret; only the holder of the
decapsulation key can recover it. There is no joint computation — the
encapsulator generates the secret unilaterally and transmits it encrypted.

The ML-KEM-768 key sizes are governed by the ring dimension and modulus:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Object*], [*Size*], [*Role*],
    [Encapsulation key (public)], [1184 B], [Distributed to the party that will encapsulate secrets.],
    [Decapsulation key seed],     [64 B],   [Stored securely by the key holder; full decapsulation key is derived on demand.],
    [Ciphertext],                 [1088 B], [Transmitted from encapsulator to decapsulator; contains the encrypted shared secret.],
    [Shared secret],              [32 B],   [Recovered by both parties; used as input to HKDF for session key derivation.],
  ),
  caption: [ML-KEM-768 key and ciphertext sizes],
) <tab-ml-kem-sizes>

The encapsulation operation is $"Encaps"("ek") -> ("ct", "ss")$: given the
encapsulation key, it draws random coins, computes a ciphertext, and outputs a
shared secret. Decapsulation is $"Decaps"("dk", "ct") -> "ss"$: given the
decapsulation key and the ciphertext, it recovers the same shared secret.
ML-KEM provides IND-CCA2 security (indistinguishability under adaptive chosen
ciphertext attacks) under the MLWE assumption.

==== ML-DSA-65: Lattice-Based Digital Signatures

ML-DSA (formerly CRYSTALS-Dilithium) is a Fiat-Shamir with Aborts signature
scheme. Signing generates a candidate response vector and rejects it if it
leaks information about the signing key, repeating until a valid response is
found. This abort-and-retry mechanism ensures that the signature statistically
hides the secret key.

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Object*], [*Size*], [*Role*],
    [Signing key seed], [32 B],   [Compact representation; full signing key is derived on demand.],
    [Verifying key],    [1952 B], [Distributed to verifiers; pre-registered in peer allowlists for PKI mode.],
    [Signature],        [3309 B], [Carried in-band in the signed key exchange extension.],
  ),
  caption: [ML-DSA-65 key and signature sizes],
) <tab-ml-dsa-sizes>

ML-DSA-65 provides existential unforgeability under chosen message attacks
(EUF-CMA) under the MLWE and MSIS (Module Short Integer Solution) assumptions,
both of which are conjectured to be hard for quantum computers.

==== Size Overhead Compared to Classical Algorithms

The quantum-resistant algorithms carry significantly larger key material and
signatures than their classical counterparts. @tab-pqc-size-comparison
summarises the size overhead, which directly affects the on-wire message sizes
for key exchange and authentication:

#figure(
  table(
    columns: (auto, auto, auto, auto),
    align: (left, left, left, left),
    [*Algorithm*], [*Role*], [*Key / encap material*], [*Sig / ciphertext*],
    [X25519],     [ECDH],      [32 B public key],   [—],
    [Ed25519],    [Signature], [32 B verifying key], [64 B],
    [ML-KEM-768], [KEM],       [1184 B encap key],  [1088 B ciphertext],
    [ML-DSA-65],  [Signature], [1952 B verifying key], [3309 B],
  ),
  caption: [Classical versus post-quantum algorithm sizes],
) <tab-pqc-size-comparison>

For VANET applications, the larger sizes have two consequences. First, a fully
signed ML-KEM-768 + ML-DSA-65 key exchange message can reach approximately
6.5 KB (1197 B base + 5266 B signed extension), requiring jumbo-frame support
in the underlying transport — vigilant-parakeet sets `PACKET_BUFFER_SIZE =
9000` bytes to accommodate this. Second, the key exchange latency budget must
account for the larger message traversal time across the wireless link.
Critically, the overhead applies only to the session setup handshake; subsequent
encrypted data frames carry only the 28-byte AEAD header (12-byte nonce +
16-byte tag) regardless of which key exchange algorithm was used.

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

The long operational lifetime of vehicular infrastructure introduces an
additional dimension: the *harvest now, decrypt later* threat. An adversary
who records today's encrypted VANET traffic can store it and attempt decryption
once quantum hardware capable of running Shor's algorithm @shor94 becomes
available. For traffic whose confidentiality must be preserved beyond the
expected quantum horizon, key exchange algorithms resistant to quantum attacks
— such as ML-KEM-768 @fips203 — are necessary even before large-scale quantum
computers exist.

=== Replay Window Mechanism

The *sliding receive window* is the standard technique for replay protection
in authenticated packet protocols, used in IPsec AH @ipsec-ah and IKEv2, and
implemented in vigilant-parakeet's `ReplayWindow` for HeartbeatReply messages.
The mechanism warrants a brief treatment here because the vigilant-parakeet
implementation follows the IPsec design closely, including a non-obvious
extension for window-poisoning prevention.

The core state is a pair `(last_seq: u32, window: u64)`:

- `last_seq` is the highest accepted sequence number seen so far.
- `window` is a 64-bit bitmask: bit $i$ represents whether sequence number
  `last_seq - i` has been accepted (`1` = accepted; `0` = not yet seen).

A received packet with sequence number `seq` is processed as follows:

+ If `seq > last_seq + 1`: the packet is too far ahead. It is accepted; the
  bitmask is left-shifted by `seq - last_seq` (clearing bits that shifted
  off the low end), `last_seq` is updated to `seq`, and bit 0 is set.

+ If `seq == last_seq + 1`: the packet is the expected next sequence number.
  Accept, update `last_seq`, set bit 0.

+ If `last_seq - 64 < seq <= last_seq` (within the window): check bit
  `last_seq - seq`. If the bit is already set, the packet is a duplicate:
  drop. If the bit is clear, accept and set it.

+ If `seq <= last_seq - 64` (beyond the left edge of the window): drop.
  The packet is too old to verify uniqueness.

The bitmask efficiently represents the acceptance state of the last 64 sequence
numbers in a single 8-byte integer. IPsec AH specifies a mandatory minimum
window size of 32 and recommends 64; vigilant-parakeet uses 64.

The *window-poisoning attack* is a subtle vulnerability in naive implementations:
an attacker forges a HeartbeatReply with `seq = u32::MAX` (or any value far
above the current `last_seq`). The window shifts by `u32::MAX - last_seq`,
advancing `last_seq` to `u32::MAX`. All subsequent legitimate replies with
small sequence numbers (including `last_seq + 1` after wrap) now fall
entirely outside the window and are dropped as "too old." The RSU's routing
table stops updating for that sender.

IPsec counters this by rejecting packets whose sequence number is implausibly
large relative to the expected next sequence number. vigilant-parakeet's defence
is different and specific to the heartbeat context: the RSU maintains a
ring buffer of the last $N$ sequence numbers it *sent* in its own Heartbeats.
Before calling `check_and_update()` on the replay window, the RSU verifies
that the received `seq` appears in its sent-heartbeat ring. A forged large
sequence number that was never sent is rejected at this pre-check step,
before it can poison the window.

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

==== Ownership, Borrowing, and Memory Safety

Rust's central innovation is its *ownership* discipline enforced at compile
time by the borrow checker. Every value has a single owner; when the owner
goes out of scope, the value is dropped (its destructor runs and memory is
freed) without a garbage collector. Shared read-only access is provided by
*shared references* (`&T`), exclusive mutable access by *mutable references*
(`&mut T`). The borrow checker ensures that at most one mutable reference or
any number of shared references to a value can exist at any time — the
"aliasing XOR mutability" invariant.

For network protocol code, this eliminates:
- *Buffer overflows*: slice indexing is bounds-checked; out-of-bounds access
  panics or returns an `Option::None` rather than reading adjacent memory.
- *Use-after-free*: the owner of a resource holds the sole live reference; all
  borrowed references are statically proven to not outlive the owner.
- *Data races*: mutable access to shared data requires mutual exclusion
  (`Mutex<T>`, `RwLock<T>`); the compiler rejects code that shares `&mut T`
  across threads without synchronisation. The `Send` and `Sync` marker traits
  enforce this at the type level.

For a VANET simulator where multiple nodes share routing state via
`Arc<RwLock<RoutingTable>>`, these guarantees mean that deadlocks and data
races are the programmer's last concern rather than a constant source of
debugging. The integration test suite runs under address sanitiser in CI
(`RUSTFLAGS="-Z sanitizer=address"`) with no false positives expected, because
the borrow checker has already excluded the bugs that sanitisers catch.

==== Async/Await and the Tokio Runtime

Tokio implements a cooperative, multi-threaded async executor. An `async fn`
in Rust desugars to a state machine implementing the `Future` trait: each
`.await` point is a yield point at which the task can be suspended and another
task scheduled. The key properties relevant to vigilant-parakeet are:

*M:N thread model.* Tokio maps many Tokio tasks (logical coroutines) onto a
smaller, configurable pool of OS threads (default: one per logical CPU). On
the evaluation machine (24 threads), dozens of node tasks share 24 OS threads
without per-task OS-thread overhead. The memory overhead of a Tokio task is
the size of its captured state (typically a few hundred bytes for a simple I/O
loop) rather than the 2–8 MB stack of an OS thread.

*I/O without blocking.* TUN read operations are wrapped with
`tokio::io::AsyncReadExt`; when no frame is available, the task yields, freeing
its OS thread for other tasks. A single OS thread can multiplex reads from
many TUN interfaces simultaneously.

*Mocked time.* `tokio::time::pause()` stops the Tokio time source;
`tokio::time::advance(duration)` advances it by the given amount without
wall-clock delay. This allows integration tests to simulate 12 hours of
re-keying cycles in milliseconds, and to test latency-measurement logic with
precise, reproducible timing.

*Work stealing.* Tokio uses a work-stealing scheduler: if one OS thread runs
out of tasks while another has a backlog, it steals tasks from the other's
queue. This makes throughput robust to uneven task distribution without manual
thread assignment.

==== Compile-Time Zero-Cost Abstractions

Rust's generics and trait system enable abstractions that have zero runtime
cost (no virtual dispatch, no heap allocation) when the concrete type is known
at compile time. The `Tun` and `Device` traits in `common` are monomorphised:
in production code, `create<T: Tun>(...)` is instantiated with the real
`TokioTun` or `tun_tap::Iface` types, and the compiler inlines all trait method
calls. In test code, the same generic is instantiated with the shim
implementation backed by Tokio channels. The result is that the test and
production code paths are structurally identical — the compiler can verify both
— but generate completely different machine code appropriate to each context.
