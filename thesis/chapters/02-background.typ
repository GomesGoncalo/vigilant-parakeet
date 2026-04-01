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

A survey of routing approaches in VANETs @vanet-routing-survey identifies
three broad families: topology-based (proactive and reactive), position-based,
and infrastructure-assisted beacon routing. Each presents different trade-offs
under vehicular channel conditions.

=== Topology-Based Routing

*Proactive* protocols maintain routing tables continuously via periodic
control exchanges. DSDV (Destination-Sequenced Distance-Vector) @dsdv extends
Bellman-Ford with sequence-numbered advertisements to eliminate count-to-infinity
loops. OLSR (Optimised Link State Routing) @olsr, an IETF standard for MANETs,
reduces link-state flooding overhead through *multipoint relay* (MPR) selection:
each node designates a minimal subset of its neighbours to retransmit its
topology updates, pruning the broadcast spanning tree. Both protocols incur
ongoing control overhead proportional to the number of nodes, which can be
prohibitive at vehicular densities.

*Reactive* protocols generate routes on demand, reducing control overhead
at the cost of discovery latency. AODV (Ad-hoc On-demand Distance Vector)
@aodv floods route-request messages and caches discovered routes until
invalidation. DSR (Dynamic Source Routing) @dsr embeds the complete path in
the packet header, eliminating intermediate routing tables but increasing
header size. In vehicular settings, the route discovery round-trip can exceed
the contact window with a given RSU, making reactive protocols unsuitable for
delay-sensitive applications.

=== Position-Based Routing

Geographic routing such as GPSR (Greedy Perimeter Stateless Routing) @gpsr
exploits GPS coordinates to forward packets toward a destination without
maintaining global topology state. Each hop greedily selects the neighbour
geographically closest to the destination; perimeter mode activates when no
such neighbour exists. While bandwidth-efficient, GPSR requires accurate,
frequently updated position information and degrades in sparse networks where
the greedy condition fails persistently.

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

== Security in Vehicular Networks

=== Cryptography primer

A brief mathematical summary of the cryptographic primitives used in this project:

* X25519 (Curve25519 Diffie–Hellman): an elliptic-curve Diffie–Hellman (ECDH) key exchange over the Montgomery curve Curve25519. Each party generates a 32-byte scalar private key a and computes the 32-byte public key A = a ⋅ G, where G is the curve base point. Given public keys A and B and private scalars a and b, both parties compute the shared secret S = a ⋅ B = b ⋅ A. The operation is constant-time and provides 128-bit security.

* HKDF (HMAC-based Key Derivation Function): HKDF-Extract(salt, IKM) = HMAC(salt, IKM) produces a pseudorandom key (PRK). HKDF-Expand(PRK, info, L) repeatedly uses HMAC to produce L bytes of output. HKDF ensures that the raw shared secret from X25519 is stretched and domain-separated into symmetric keys of the correct length.

* AEAD (Authenticated Encryption with Associated Data): AEAD constructions (AES-GCM, ChaCha20-Poly1305) provide both confidentiality and integrity. Encryption outputs ciphertext and an authentication tag computed over the ciphertext and AAD; on decryption the tag is verified and the plaintext is rejected if verification fails. Nonce uniqueness is required: this implementation uses a 12-byte nonce from a CSPRNG for each encryption to avoid nonce reuse.

* Ed25519 signatures: Ed25519 is an EdDSA signature scheme over the Edwards25519 curve. Given a message m and signing key sk, the signer computes a deterministic nonce and produces a 64-byte signature sig; verification uses the public key pk and either accepts or rejects sig. Here Ed25519 is used optionally to authenticate DH handshake messages: the signed base payload (42 bytes) excludes the appended signature fields to permit a fixed-length base format.

These primitives are combined as follows: X25519 produces a shared secret; HKDF extracts and expands it into keys for the chosen AEAD cipher; AEAD encrypts application payloads; Ed25519, when enabled, signs the handshake base payload to prevent active man-in-the-middle attacks.

== Simulation Approaches

=== Threat Taxonomy

Vehicular networks are exposed to a range of adversarial behaviours that span
both the data plane (payload forwarding) and the control plane (routing):

- *Routing table poisoning*: an adversary controlling one or more intermediate
  nodes selectively drops, replays, or crafts heartbeat messages to corrupt
  routing state across a wide area, attracting traffic through attacker-controlled
  paths. @l3-security-vehicular provides a detailed analysis of this class of
  attack in the OBU/RSU setting.

- *Black hole and grey hole attacks*: a node advertises an artificially
  attractive route then silently drops (black hole) or probabilistically
  forwards (grey hole) the attracted traffic, acting as a selective packet
  sink.

- *Sybil attacks* @sybil: a single physical node presents multiple false
  MAC or IP identities, polluting routing tables and consensus mechanisms
  with phantom participants. Sybil identities are particularly harmful in
  heartbeat-based routing because each fake identity can independently
  attract traffic.

- *Wormhole attacks* @wormhole: two colluding nodes tunnel packets between
  distant network segments out-of-band, creating the illusion of a
  low-latency shortcut. Neighbours at both tunnel endpoints observe
  apparently direct connectivity to each other, preferring the wormhole
  path; the colluding pair can then selectively manipulate forwarded traffic.

- *Replay attacks*: previously captured control messages (e.g., heartbeats
  with high sequence numbers) are re-injected to maintain stale routing
  entries or to exhaust sequence counters.

- *Passive eavesdropping*: a physically present adversary captures payload
  frames from any link, recovering application data from unencrypted
  transmissions.

=== Standardised Security Frameworks

IEEE 1609.2 @ieee-1609-2 defines the security architecture for Wireless
Access in Vehicular Environments (WAVE). Its core mechanisms are:

- *Signed messages*: every safety-critical Wireless Short Message (WSM) is
  signed with ECDSA over P-256 or P-384, binding it to a pseudonymous
  certificate issued by a certificate authority.

- *Pseudonymous certificates*: vehicles rotate through a pool of short-lived
  certificates to prevent long-term tracking while retaining revocability.

- *Certificate revocation*: compromised certificates are invalidated via
  CRL distribution or OCSP, removing misbehaving nodes from the network
  without requiring their cooperation.

The ETSI ITS security framework @etsi-its complements IEEE 1609.2 with
Authorisation Ticket (AT) mechanisms for Europe. A common challenge across
both standards is key management at scale: certificate distribution,
revocation, and renewal must operate under intermittent connectivity and
strict latency budgets.

=== Broadcast Authentication

Heartbeat and beacon messages are inherently broadcast, which complicates
authentication: a receiver cannot know in advance the sender's public key to
verify an asymmetric signature, and distributing a group key for HMAC creates
revocation challenges. Two approaches from the literature are relevant:

The *TESLA* protocol @tesla uses a time-delayed key disclosure mechanism based
on a one-way hash chain. The sender commits to a key chain at broadcast time
and discloses keys one period later; receivers buffer messages and
retroactively verify authenticity once the key is released. TESLA provides
broadcast authentication without per-receiver key agreement, but introduces
an authentication latency of one heartbeat period — a cost that must be
weighed against the routing-stability benefit.

*HMAC* @rfc2104 provides integrity and authenticity on unicast paths where a
shared symmetric key can be established beforehand. HMAC is suitable for the
OBU–server unicast channel once a DH session key exists, but not for
unauthenticated broadcast heartbeats.

=== Data Confidentiality

Early vehicular network designs prioritised availability and integrity (safety
messages must arrive and must not be modified), treating payload confidentiality
as secondary. As VANETs expand to carry non-safety traffic — internet access,
toll transactions, content delivery — protecting payload data from relay nodes
and eavesdroppers becomes significant.

The standard approach is *end-to-end encryption* at the OBU–server boundary:
intermediate relay nodes (other OBUs, RSUs) forward ciphertext opaquely and
never hold session keys. The central challenge in the VANET context is completing
a key exchange within the contact window an OBU may have with a given RSU.
Two-message Diffie-Hellman handshakes @x25519 are lightweight enough to
complete in a single RSU contact and produce a forward-secret session key
without requiring certificate infrastructure. Raya and Hubaux @raya-hubaux
analyse the broader trade-offs between pseudonymous certificate-based
approaches and lightweight alternatives for vehicular security.

=== The Reference Security Model

The paper by @l3-security-vehicular specifically analyses Layer-3 security
threats in vehicular networks, with a focus on routing manipulation attacks
in the OBU/RSU architecture. A central finding is that an adversary
controlling one or more intermediate nodes can selectively drop, replay, or
modify heartbeat messages to poison routing tables across a wide area — an
attack that is efficient precisely because heartbeats carry no authentication.
The routing protocol implemented in vigilant-parakeet is designed to reproduce
the network model described in that work and serve as a platform for studying
both the attacks and the defences proposed therein.

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
