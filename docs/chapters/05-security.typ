// ── Chapter 5 — Security Architecture <security> ─────────────────────────────
#import "@preview/fletcher:0.5.7" as fletcher: diagram, node, edge
#import "@preview/chronos:0.2.1"

= Security Architecture <security>

A key motivation for this project is the study of Layer-3 security in
vehicular networks @l3-security-vehicular. vigilant-parakeet therefore
implements a full end-to-end security layer enabling OBUs to establish
encrypted sessions with a backend server without trusting intermediate
RSUs or relay OBUs.

== Threat Model

The security model assumes the following adversary capabilities:

- *Untrusted relay nodes*: intermediate OBUs and even RSUs may be
  compromised. They must not be able to read or tamper with payload data
  exchanged between a legitimate OBU and the server.

- *Passive eavesdroppers*: an attacker can capture all frames on any link
  in the network.

- *Active man-in-the-middle*: an adversary positioned between the OBU and
  the server can intercept and modify in-flight key exchange messages. In
  the absence of authentication, this allows the attacker to substitute its
  own public key and establish independent DH sessions with each endpoint,
  recovering plaintext as a transparent relay.

- *Routing manipulation*: as analysed in @l3-security-vehicular, an attacker
  controlling intermediate nodes can inject or replay heartbeat control messages
  to poison OBU routing tables, steering traffic through attacker-controlled
  paths without breaking the encryption layer.

- *Quantum adversary*: a computationally unbounded adversary equipped with a
  large-scale quantum computer can break X25519 and Ed25519 using Shor's
  algorithm @shor94, recovering session keys from stored ciphertext or forging
  handshake signatures. The *harvest now, decrypt later* (HNDL) attack —
  capturing traffic today for quantum-assisted decryption in the future — is a
  present concern given the long deployment lifetimes of vehicular
  infrastructure. ML-KEM-768 and ML-DSA-65 are provided as drop-in
  replacements for X25519 and Ed25519 respectively, restoring security under
  the Module Learning With Errors (MLWE) hardness assumption (see @sec-pqc).

The design consequence is *end-to-end encryption at the OBU–server boundary*:
RSUs and relay OBUs forward encrypted payloads opaquely and never hold the
session key. The MitM threat is addressed by an optional Ed25519 authentication
layer on the key exchange messages (see @sec-dh-auth). Control-plane heartbeat
authentication is identified as future work (@conclusion).

== Configurable Cipher Suite

All cryptographic parameters are encapsulated in `CryptoConfig`
(defined in `node_lib::crypto`):

```rust
pub struct CryptoConfig {
    pub cipher:            SymmetricCipher,   // default: AES-256-GCM
    pub kdf:               KdfAlgorithm,      // default: HKDF-SHA256
    pub dh_group:          DhGroup,           // default: X25519
    pub signing_algorithm: SigningAlgorithm,  // default: Ed25519
}
```

Each axis is independently configurable via node YAML:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Parameter*], [*Options*], [*Notes*],
    [`cipher`],
      [`aes-256-gcm` (default)\ `aes-128-gcm`\ `chacha20-poly1305`],
      [All three are AEAD ciphers providing both confidentiality and integrity. ChaCha20-Poly1305 is preferred on hardware without AES-NI.],
    [`kdf`],
      [`hkdf-sha256` (default)\ `hkdf-sha384`\ `hkdf-sha512`],
      [HKDF @hkdf extracts and expands the raw DH shared secret into a uniformly random key of the required length.],
    [`dh_group`],
      [`x25519` (default)\ `ml-kem-768`],
      [X25519 (Curve25519 @x25519) provides 128-bit classical security. ML-KEM-768 (NIST FIPS 203 @fips203) is the quantum-resistant alternative, providing security against attacks by quantum computers.],
    [`signing_algorithm`],
      [`ed25519` (default)\ `ml-dsa-65`],
      [Ed25519 @rfc8032 is the classical signing algorithm. ML-DSA-65 (NIST FIPS 204 @fips204) is the quantum-resistant alternative. The signing algorithm applies to both key exchange authentication and session revocation.],
  ),
  caption: [Configurable cipher suite parameters],
) <tab-cipher-suite>

== Key Exchange Protocol

Key exchange follows a two-message Diffie-Hellman handshake carried over
the existing control-plane message infrastructure.

=== Wire Format

Two authentication message types are defined in
`node_lib::messages::auth`. Both `KeyExchangeInit` and
`KeyExchangeReply` share the same variable-length layout; the distinguishing
information is the sub-type discriminant within the `Auth` packet family
(`PacketType::Auth`, wire byte `0x02`). An `algo_id` byte at the start of
each message identifies the key-exchange algorithm in use, enabling receivers
to parse key material of the correct length. The `algo_id` wire constants
(`0x01` for X25519, `0x02` for ML-KEM-768) are internal to the `auth` module;
outside callers receive and supply a typed `DhGroup` value through the
`dh_group()` accessor and the `new_unsigned()` / `new_signed()` constructors.

*Unsigned base format*:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Field*], [*Size*], [*Description*],
    [`algo_id`],          [1 B],   [Key-exchange algorithm: `0x01` = X25519, `0x02` = ML-KEM-768.],
    [`key_id`],           [4 B],   [Monotonically increasing exchange identifier, used as HKDF salt to bind the derived key to this round.],
    [`key_material_len`], [2 B BE],[Length of the following key material field.],
    [`key_material`],     [var],   [For X25519: 32-byte ephemeral public key. For ML-KEM-768 Init: 1184-byte encapsulation key. For ML-KEM-768 Reply: 1088-byte ciphertext.],
    [`sender`],           [6 B],   [MAC address of the originating node.],
  ),
  caption: [Unsigned `KeyExchangeInit` / `KeyExchangeReply` base format (variable length)],
) <tab-ke-wire>

Total unsigned size: 45 bytes for X25519 (1+4+2+32+6), 1197 bytes for
ML-KEM-768 Init (1+4+2+1184+6), and 1101 bytes for ML-KEM-768 Reply
(1+4+2+1088+6).

*Signed extension*: when authentication is enabled (see @sec-dh-auth), a
variable-length extension is appended after the base payload:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Field*], [*Size*], [*Description*],
    [`sig_algo_id`],     [1 B],   [Signing algorithm: `0x01` = Ed25519, `0x02` = ML-DSA-65.],
    [`spk_len`],         [2 B BE],[Length of the signing public key.],
    [`signing_pubkey`],  [var],   [Ed25519: 32 bytes. ML-DSA-65: 1952 bytes.],
    [`sig_len`],         [2 B BE],[Length of the signature.],
    [`signature`],       [var],   [Signature over the base payload. Ed25519: 64 bytes. ML-DSA-65: 3309 bytes.],
  ),
  caption: [Signed extension appended to `KeyExchangeInit` / `KeyExchangeReply`],
) <tab-ke-signed-wire>

The `TryFrom<&[u8]>` deserialiser reads `algo_id` and `key_material_len`
first, then parses the key material of the declared length, followed by
the 6-byte sender. If bytes remain, the signed extension is parsed and
the `sig_algo_id` is validated against the known Ed25519 and ML-DSA-65
sizes; any mismatch is rejected. The cloud-protocol `KeyExchangeForward`
(type `0x04`) and `KeyExchangeResponse` (type `0x05`) messages carry the
raw payload opaquely, so they are algorithm-agnostic.

=== Handshake Flow

The handshake is a two-message exchange, but the mechanics differ by
algorithm:

==== X25519 (Classical ECDH)

+ The OBU generates an ephemeral X25519 keypair, assigns the next
  `key_id`, stores a `PendingExchange`, and sends `KeyExchangeInit`
  carrying its 32-byte public key toward the server.

+ The RSU forwards the message opaquely as a `KeyExchangeForward` UDP
  datagram (type `0x04`) to the server.

+ The server generates its own ephemeral X25519 keypair, computes
  `DH(server_secret, obu_public)`, derives the session key via HKDF,
  stores it, and returns its 32-byte public key in `KeyExchangeReply`.

+ The OBU receives the reply, computes
  `DH(obu_secret, server_public)`, derives the same session key via
  HKDF, and moves the entry to `established`.

Both sides arrive at the same shared secret without transmitting any
private key material.

==== ML-KEM-768 (Quantum-Resistant KEM, NIST FIPS 203)

ML-KEM-768 is a Key Encapsulation Mechanism (KEM): rather than two
parties performing a joint Diffie-Hellman computation, one party
(the server) *encapsulates* a fresh random secret under the other's
public key (the encapsulation key), and the holder of the corresponding
decapsulation key recovers the secret.

Its security rests on the *Module Learning With Errors* (MLWE) problem
(see @sec-pqc): an attacker who observes the encapsulation key and the
ciphertext must solve an instance of MLWE to recover the shared secret, a
problem for which no efficient classical or quantum algorithm is known. This
gives ML-KEM-768 IND-CCA2 security at NIST Security Level 3 — comparable to
breaking AES-192 — under the MLWE assumption.

+ The OBU generates an ML-KEM-768 keypair, retains the 64-byte
  decapsulation-key seed, and sends its 1184-byte *encapsulation key*
  in `KeyExchangeInit`.

+ The server calls `kem_768_encapsulate(encap_key)`, which produces a
  1088-byte *ciphertext* and a 32-byte shared secret. The server derives
  the session key from the shared secret via HKDF and sends the
  ciphertext in `KeyExchangeReply`.

+ The OBU reconstructs its decapsulation key from the stored seed and
  calls `kem_768_decapsulate(seed, ciphertext)` to recover the same
  32-byte shared secret, then derives the session key via HKDF.

The shared secret is chosen by the server and sent to the OBU encrypted under
its encapsulation key. A passive or active attacker without the decapsulation
key cannot recover the secret even with a quantum computer, because breaking
the ciphertext requires solving MLWE. Note the role asymmetry compared to
X25519: the OBU cannot verify that the encapsulated secret was chosen freshly
(rather than replayed) without the authenticated handshake extension
(@sec-dh-auth), making authentication especially important in the ML-KEM mode.

#figure(
  scale(70%, reflow: true, chronos.diagram({
    import chronos: *
    _par("OBU")
    _par("RSU", display-name: "RSU (relay)")
    _par("Server")
    _seq("OBU", "RSU", comment: "KeyExchangeInit (encap_key / pubkey_A)")
    _seq("RSU", "Server", comment: "KeyExchangeForward")
    _seq("Server", "Server", comment: "X25519: gen pub_B; DH→ss | ML-KEM: encapsulate(ek)→(ct,ss)")
    _seq("Server", "RSU", comment: "KeyExchangeResponse")
    _seq("RSU", "OBU", comment: "KeyExchangeReply (pubkey_B / ciphertext)")
    _seq("OBU", "OBU", comment: "X25519: DH→ss | ML-KEM: decapsulate(ct)→ss")
    _sep("Encrypted channel established")
    _seq("OBU", "Server", comment: "Encrypted data")
  }, width: 150mm)),
  caption: [Key exchange handshake (unsigned): X25519 ECDH or ML-KEM-768 KEM, and subsequent encrypted data flow],
) <fig-dh-handshake>

== Key Derivation

== Key-exchange robustness

Several practical improvements were implemented to increase the reliability of
Key Exchange in the presence of loss and churn:

+ Downstream Client Cache: RSUs maintain a small per-OBU client cache that
  records recent downstream recipients. KeyExchangeReply messages are routed
  via this ClientCache to ensure replies are forwarded to the correct downstream
  interface even when the immediate upstream has changed since the Init was sent.

+ Reply routing and forwarding fixes: KeyExchangeReply forwarding was hardened
  to avoid loops and to prefer cached downstream entries when available. These
  fixes reduce stray reply drops and improve session establishment reliability
  in multi-hop scenarios.

+ Prompt retry behaviour: OBUs aggressively retry DH/ KEM exchange attempts at
  startup when no upstream is yet cached, and after short send-timeouts during
  early boot. This reduces time-to-establish in high-loss scenarios or when the
  initial route is not yet stabilised.

These robustness features are documented in the code (`obu_lib::control::mod`,
`rsu_lib::control::mod`) and exposed via the admin console for debugging (the
`sessions` and `routes` commands).


The raw shared secret — 32 bytes from either X25519 or ML-KEM-768 — is not
used directly as a symmetric key. Instead, it is passed through HKDF @hkdf:

```
HKDF-Expand(
    PRK  = HKDF-Extract(salt, shared_secret),
    info = b"vigilant-parakeet-dh",
    L    = cipher.key_len()    // 16 (AES-128) or 32 (AES-256, ChaCha20)
)
```

The HKDF salt is constructed as:
`b"vigilant-parakeet-salt-" ‖ key_id.to_be_bytes()`, which ensures that
re-keying (incrementing `key_id`) produces a cryptographically independent
key even if the same keypair were reused. Using HKDF as a post-processing
step is particularly important for ML-KEM-768: the KEM shared secret is
pseudorandom by construction, but routing it through HKDF ensures uniform
key material regardless of any subtle biases in the KEM output and allows the
same derivation code path for both algorithms.

== Symmetric Encryption

Payload encryption uses AEAD (Authenticated Encryption with Associated
Data). The wire format for every encrypted buffer is:

#figure(
  table(
    columns: (auto, 1fr, auto),
    align: (center, center, center),
    [`nonce (12 B)`], [`ciphertext (variable)`], [`auth tag (16 B)`],
  ),
  caption: [AEAD encrypted buffer format; total overhead: 28 bytes],
)

The 12-byte nonce is generated freshly from the OS CSPRNG for every
encryption, ensuring that nonce reuse is eliminated even under high
traffic rates. The 16-byte GCM/Poly1305 authentication tag provides
integrity and authenticity; decryption fails immediately if the tag does
not verify, preventing tampering by any relay node.

The encryption path in `obu_lib::control` is:

```rust
// Upstream: OBU → server
let encrypted = encrypt_with_config(cipher, plaintext, &dh_key)?;

// Downstream: server → OBU
let plaintext = decrypt_with_config(cipher, buf.data(), &dh_key)?;
```

If no established key exists for the server (handshake not yet complete),
encryption is skipped and the frame is held or dropped depending on
configuration.

== DH Key Store Lifecycle

The `DhKeyStore` in `obu_lib::control::dh_key_store` manages key state
as a simple state machine (@fig-dh-state):

#figure(
  diagram(
    node-stroke: 0.5pt,
    spacing: (25mm, 20mm),
    node((0,0), [None], name: <none>),
    node((2,-2), [Pending], name: <pending>),
    node((2,2), [Established], name: <established>),
    edge(<none>, <pending>, "->", [initiate\_exchange()], label-side: left),
    edge(<pending>, <established>, "->", [complete\_exchange()\ (key\_id match)], label-side: left),
    edge(<established>, <none>, "->",
      [is\_key\_expired() → true\ or remove\_pending() (max retries)],
      label-side: left),
  ),
  caption: [DH key store state machine per peer],
) <fig-dh-state>

Key lifecycle features:

- *Periodic rekeying*: the OBU runs a background rekey task that wakes every
  `dh_rekey_interval_ms` and evaluates whether a new key exchange should be
  started. This periodic timer implements *proactive* re-keying independent of
  observed traffic patterns, preventing very-long-lived session keys. The
  rekeying interval is configurable via node YAML (`dh_rekey_interval_ms`).

- *Timeout and retry*: each pending exchange records the time it was
  initiated (`initiated_at`) and a retry counter. If no `KeyExchangeReply`
  arrives within the configured `reply_timeout_ms` (derived from runtime
  parameters), `is_pending_timed_out()` returns true and the OBU calls
  `reinitiate_exchange()`. `reinitiate_exchange()` preserves the previous
  retry count and stores the incremented `retries` value with the new pending
  entry, enabling diagnostic visibility into repeated failures.

- *Forced rekey (administrative or server-driven)*: the OBU exposes a local
  admin command (`rekey`) and also responds to server-sent `SessionTerminated`
  notices. Both paths notify the rekeying task (via an `Arc<Notify>` called
  `rekey_notify`) so the OBU performs an immediate re-key outside the normal
  periodic schedule.

- *Key expiry*: an established key holds an `established_at` timestamp. The
  helper `is_key_expired()` compares the elapsed time since `established_at`
  against `dh_key_lifetime_ms`; expired keys are proactively rotated. This
  provides bounded forward secrecy and limits the window during which a
  compromised session key is valid.

- *Key ID uniqueness and wrap*: each outgoing exchange is assigned a
  monotonically increasing `next_key_id`. When `next_key_id` wraps, the
  implementation uses wrapping arithmetic but relies on the key lifetime and
  sequence checks to prevent replay/poisoning across wraps.

- *Key ID mismatch rejection and single-use pending state*: `complete_exchange()`
  checks that the received `key_id` exactly matches the pending exchange's
  `key_id` before consuming the pending entry. If the `key_id` differs, the
  reply is ignored. Pending key material (X25519 EphemeralSecret) is consumed
  on completion to prevent accidental reuse; ML-KEM pending state stores only a
  compact decapsulation seed which is zeroized on removal.

- *Telemetry*: when a session is established the code records the key
  establishment latency (elapsed ms) and the number of retries; the
  established key is stored along with `established_at` so admin commands and
  metrics can report key age (`get_session_info()` returns `(key_id, age_secs)`).

== Authentication for Key Exchange <sec-dh-auth>

The unauthenticated handshake described above is vulnerable to an active
man-in-the-middle attack: an adversary positioned between the OBU and the
server can intercept `KeyExchangeInit`, substitute its own key material,
complete a separate exchange with the server, and relay decrypted/re-encrypted
traffic transparently. Neither endpoint can detect the substitution from the
key bytes alone.

vigilant-parakeet addresses this with an optional *digital signature layer*
applied to both handshake messages. Two signature algorithms are supported:
the classical *Ed25519* @rfc8032 and the quantum-resistant *ML-DSA-65* (NIST
FIPS 204 @fips204). The signature is carried in-band in the signed extension
described in @tab-ke-signed-wire.

ML-DSA-65 provides the same role as Ed25519 — binding a handshake message to a
node's long-term identity — but with security against quantum adversaries. Where
Ed25519 is an elliptic-curve scheme broken by Shor's algorithm @shor94, ML-DSA-65
is a Fiat-Shamir with Aborts lattice scheme whose security reduces to the MLWE
and Module Short Integer Solution (MSIS) problems (see @sec-pqc). The cost is a
significant increase in key and signature size: an Ed25519 verifying key is 32
bytes and a signature is 64 bytes, whereas ML-DSA-65 requires 1952 bytes for the
verifying key and 3309 bytes for the signature (see @tab-signing-sizes). When
combined with ML-KEM-768 key material, a fully signed quantum-resistant key
exchange message can reach approximately 6.5 KB; the 9000-byte packet buffer
accommodates this.

=== Signing Identity

Each participating node holds a long-lived signing identity keypair,
encapsulated in `SigningKeypair` (`node_lib::crypto`):

```rust
pub struct SigningKeypair {
    seed: Zeroizing<[u8; 32]>,
    inner: SigningKeypairInner,  // Ed25519 or ML-DSA-65
}
```

The signing key serves as a stable node identity across sessions, analogous
to an SSH host key. Two construction modes are supported:

- *Ephemeral*: `SigningKeypair::generate(algo)` draws 32 bytes from the OS
  CSPRNG and creates a fresh keypair. Convenient for testing; the identity is
  lost on restart.

- *Persistent*: `SigningKeypair::from_seed(seed: [u8; 32], algo)` derives
  the same keypair deterministically from a fixed seed. The seed is stored in
  node YAML under `signing_key_seed` (64 hex characters). The verifying key
  is stable across restarts, enabling pre-registration in peer allowlists.

The seed is wrapped in `Zeroizing<[u8; 32]>` so the key material is wiped
from memory when the keypair is dropped.

At startup a node with `enable_dh_signatures: true` logs its verifying
(public) key in hexadecimal. A companion `keygen` binary is provided for
generating seed/verifying-key pairs at provisioning time:

```sh
$ keygen generate ed25519
seed=<64 hex chars>   verifying_key=<64 hex chars>

$ keygen generate ml-dsa-65
seed=<64 hex chars>   verifying_key=<3904 hex chars>
```

Key sizes by algorithm:

#figure(
  table(
    columns: (auto, auto, auto),
    align: (left, left, left),
    [*Algorithm*], [*Verifying key*], [*Signature*],
    [Ed25519],   [32 B (64 hex chars)],   [64 B],
    [ML-DSA-65], [1952 B (3904 hex chars)], [3309 B],
  ),
  caption: [Signing algorithm key and signature sizes],
) <tab-signing-sizes>

=== Signature Computation and Verification

The signature covers the *base payload* of the handshake message —
`algo_id | key_id | key_material_len | key_material | sender` — binding
the authentication to the specific exchange round without committing to
the appended signature fields:

```rust
let base = ke_init.base_payload();
let sig  = signing_keypair.sign(&base);
```

Verification dispatches on the `signing_algorithm()` accessor (which maps the
internal `sig_algo_id` wire byte to `SigningAlgorithm::Ed25519` or
`SigningAlgorithm::MlDsa65`) to the appropriate algorithm.
A failed verification returns `NodeError::SignatureError` and causes the
handshake message to be silently dropped.

=== Authenticated Handshake Flow

#figure(
  scale(70%, reflow: true, chronos.diagram({
    import chronos: *
    _par("OBU")
    _par("RSU", display-name: "RSU (relay)")
    _par("Server")
    _seq("OBU", "OBU", comment: "sig_A = sign")
    _seq("OBU", "RSU", comment: "KeyExchangeInit (signed)")
    _seq("RSU", "Server", comment: "KeyExchangeForward")
    _seq("Server", "Server", comment: "verify; DH→key; sig_B")
    _seq("Server", "RSU", comment: "KeyExchangeResponse")
    _seq("RSU", "OBU", comment: "KeyExchangeReply (base_B, signing_pk_B, sig_B)")
    _seq("OBU", "OBU", comment: "verify; DH→key")
    _sep("Encrypted channel established")
    _seq("OBU", "Server", comment: "Encrypted data")
  }, width: 150mm)),
  caption: [Authenticated handshake with Ed25519 or ML-DSA-65 signatures and optional PKI enforcement],
) <fig-dh-signed-handshake>

The RSU remains a transparent relay at all times: it forwards the full 138-byte
signed message without inspecting or verifying the signature. Authentication
is performed exclusively at the two endpoints.

=== Trust Models <sec-trust-models>

Two trust modes are supported, selected by configuration:

==== Trust-on-First-Use (TOFU)

In TOFU mode (`enable_dh_signatures: true`, no `dh_signing_allowlist`
configured), the server accepts any well-formed, correctly-signed
`KeyExchangeInit`. The `signing_pubkey` carried by the first message from a
given OBU VANET MAC is implicitly trusted. On subsequent re-keys from the
same MAC, the server can detect key substitution by comparing the new
`signing_pubkey` against the value seen at first contact.

TOFU provides *post-first-contact MitM protection* at minimal operational
cost: no key pre-registration is required, and nodes can be added to the
network without coordinating with the server operator. Its limitation is
that an attacker present during the *initial* handshake can substitute a
different identity and the server has no way to detect the impersonation.

==== PKI Mode (Static Allowlist)

The server's `dh_signing_allowlist` maps each OBU VANET MAC to its
pre-registered verifying key. The key length depends on the signing
algorithm: 64 hex characters for Ed25519 (32 bytes) or 3904 hex characters
for ML-DSA-65 (1952 bytes):

```yaml
enable_dh_signatures: true
signing_algorithm: ml-dsa-65          # ed25519 or ml-dsa-65
signing_key_seed: "<64-hex-chars>"
dh_signing_allowlist:
  "AA:BB:CC:DD:EE:FF": "<3904-hex-chars>"   # OBU n2 ML-DSA-65 verifying key
  "11:22:33:44:55:66": "<3904-hex-chars>"   # OBU n3 ML-DSA-65 verifying key
```

Any `KeyExchangeInit` whose `signing_pubkey` does not exactly match the
pre-registered value for the source MAC is rejected, regardless of whether
the signature itself is well-formed. This closes the first-contact
impersonation gap at the cost of manual key provisioning.

Symmetrically, an OBU can pin the server's verifying key via
`server_signing_pubkey` in its YAML, rejecting any `KeyExchangeReply` from
a server whose signing identity does not match:

```yaml
enable_dh_signatures: true
signing_algorithm: ml-dsa-65
signing_key_seed: "<64-hex-chars>"
server_signing_pubkey: "<3904-hex-chars>"   # expected server ML-DSA-65 verifying key
```

When both sides are configured with allowlists, the handshake provides
*mutual authenticated key exchange*: an attacker cannot impersonate either
party to the other at any stage of the exchange. The security guarantee is
comparable to mutual TLS with pre-pinned certificates, but without a
certificate authority: trust is established by out-of-band key distribution
at provisioning time.

=== Security Properties Summary

@tab-auth-modes summarises the security properties under each configuration.

#figure(
  table(
    columns: (auto, auto, auto),
    align: (left, left, left),
    [*Mode*], [*Protects against*], [*Limitation*],
    [No signatures (`enable_dh_signatures: false`)],
      [Passive eavesdropping (via AEAD payload encryption)],
      [Vulnerable to active MitM on key exchange; vulnerable to quantum computers if X25519 is used.],
    [TOFU (signed, no allowlist)],
      [Post-first-contact key substitution MitM; quantum-resistant when ML-KEM-768 + ML-DSA-65 are used],
      [First-contact impersonation possible.],
    [PKI (signed + allowlist)],
      [First-contact and all subsequent impersonation; fully quantum-resistant when ML-KEM-768 + ML-DSA-65 are used],
      [Requires out-of-band key distribution; static allowlist has no revocation.],
  ),
  caption: [Security properties under each authentication configuration],
) <tab-auth-modes>

== Session Revocation <sec-session-revocation>

The server can forcibly terminate an OBU's session by sending a
`SessionTerminated` control message, which causes the target OBU to clear
its established key and immediately re-initiate a fresh key exchange.
Because this is a privileged administrative command that traverses the
VANET (RSU → OBU relay chain), it must be protected against *replay attacks*:
an attacker that captures a legitimate revocation frame could re-inject it
later to continuously disrupt an OBU's connectivity.

=== Wire Format

```
Unsigned:  [TARGET_OBU_MAC 6B]
Signed:    [TARGET_OBU_MAC 6B] [TIMESTAMP_SECS 8B] [NONCE 8B]
           [SIG_ALGO_ID 1B] [SIG_LEN 2B BE] [SIGNATURE var]
```

The server signs the payload `[0x02 | TARGET_MAC | TIMESTAMP_SECS | NONCE]`,
where `0x02` is the `SessionTerminated` sub-type byte within the `Auth`
packet family. Unsigned revocations (6-byte payload) are accepted only in
configurations where `enable_dh_signatures` is disabled.

=== Replay Prevention

Two complementary mechanisms prevent replay:

- *Timestamp*: The server embeds the current Unix time (seconds) in the
  message. The OBU rejects any `SessionTerminated` where
  `|now − timestamp| > VALIDITY_SECS` (60 seconds). A captured message
  is useless after one minute regardless of how many times it is replayed.

- *Nonce*: The server generates a fresh 8-byte random nonce for each
  revocation. The OBU maintains a time-bounded nonce cache
  (`VecDeque<(nonce, received_at)>`). On receipt of a signed revocation,
  the OBU:

  + Prunes entries older than `VALIDITY_SECS` from the front of the deque.
  + Checks whether the nonce appears anywhere in the remaining entries.
  + If found, drops the message as a replay; otherwise records the nonce
    and processes the revocation.

  Because the cache is time-bounded rather than count-bounded, old nonces
  expire automatically along with the messages they protect. The cache never
  accumulates unboundedly. A clock-skew tolerance of 5 seconds
  (`CLOCK_SKEW_TOLERANCE_SECS`) allows messages slightly in the future
  (due to loosely synchronised clocks) while still providing a bounded
  acceptance window.

Together, the timestamp closes the long-horizon replay window and the nonce
closes the short-horizon replay window within the validity period. An attacker
must forge a valid signature to mount any replay attack, which is prevented
by the Ed25519 or ML-DSA-65 authentication.

== RSU Opacity

A deliberate design property is that RSUs handle encrypted `Data` frames
*opaquely*: the RSU forwards `ToUpstream` payloads to the server
without attempting to decrypt them. This is validated by the RSU
encryption test suite, which confirms that even invalid ciphertext is
forwarded unchanged. The effect is that compromising an RSU does not
expose session keys or allow payload manipulation; only the endpoints
hold the HKDF-derived key.

== Comparison to Related Security Approaches <sec-security-comparison>

This section situates the vigilant-parakeet security architecture relative to
the three main families of VANET security solutions described in @background.

=== Comparison with IEEE 1609.2 / PKI-Based Approaches

IEEE 1609.2 @ieee-1609-2 and the ETSI ITS AT framework address the
*authentication* and *integrity* of broadcast control messages through a full
certificate infrastructure. Their primary goal is to ensure that safety
messages (CAM, DENM) cannot be injected or modified by unauthorized nodes.

vigilant-parakeet's goal is different: it addresses the *confidentiality* and
*authenticated key exchange* on the unicast OBU–server data path, for which
IEEE 1609.2 provides no direct mechanism (certificates authenticate messages
but do not establish session keys for encrypted bulk data transfer).

The two approaches are complementary:

- IEEE 1609.2 would protect *heartbeat control messages* from routing
  manipulation attacks (the threat analysed in @l3-security-vehicular) —
  this is identified as future work in @conclusion.
- vigilant-parakeet's DH + AEAD layer protects *payload data* from relay
  nodes and passive eavesdroppers — a threat IEEE 1609.2 does not address
  for unicast application traffic.

The Ed25519 authentication layer in vigilant-parakeet (@sec-dh-auth) is
philosophically similar to IEEE 1609.2 signed messages, but lightweight:
it operates on 42-byte base payloads rather than full Ethernet frames,
uses a pre-provisioned key rather than a full certificate chain, and does not
require a PKI hierarchy or certificate revocation infrastructure.

=== Comparison with TESLA-Based Broadcast Authentication

TESLA @tesla authenticates broadcast messages — heartbeats in this context —
using a one-way hash chain with time-delayed key disclosure. Integrating TESLA
into the heartbeat protocol would directly address routing table poisoning
attacks by authenticating each heartbeat message.

The key difference from vigilant-parakeet's current design is the target:
TESLA authenticates *control-plane* broadcast messages, whereas the DH + AEAD
layer authenticates and encrypts *data-plane* unicast payloads. The two
mechanisms address different threat surfaces and could coexist: TESLA on
heartbeats (future work) would remove the routing manipulation threat while the
existing DH layer continues to protect OBU–server data.

TESLA imposes an authentication delay of one disclosure interval (one heartbeat
period), which vigilant-parakeet's existing sequence number freshness check
already provides in a weaker form (rejecting replays beyond one period without
cryptographic assurance of authenticity). Full TESLA deployment would require
GPS-synchronised clocks at all OBUs to bound the bootstrapping key disclosure.

=== Comparison with End-to-End Encryption Schemes

Raya and Hubaux @raya-hubaux discuss several architectural options for
end-to-end confidentiality in VANETs, including symmetric pre-shared key
(PSK), PKI-based key encapsulation, and ephemeral DH. vigilant-parakeet
implements the ephemeral DH option, which is the most operationally flexible:

- *PSK*: requires out-of-band distribution of symmetric keys and provides no
  forward secrecy (compromise of the long-term key exposes all past sessions).

- *PKI key encapsulation*: the OBU encrypts a session key under the server's
  public certificate. Requires certificate distribution and provides one-way
  authentication (server is authenticated; OBU is not, without mutual
  certificate exchange).

- *Ephemeral DH (this work)*: forward secrecy is provided by construction
  (each session uses a fresh keypair; compromise of the Ed25519 signing key
  does not expose past session keys). Authentication is layered orthogonally via
  Ed25519 and is independently configurable (disabled, TOFU, or PKI allowlist).
  No certificate revocation infrastructure is needed; key rotation is implicit
  in the re-keying interval.

The primary advantage of the certificate-based approach is that it leverages an
existing PKI infrastructure (if one is deployed for IEEE 1609.2) without
requiring a separate key distribution mechanism. The primary advantage of the
ephemeral DH approach is that it is self-contained: it can be deployed without
any PKI infrastructure, using pre-provisioned 32-byte Ed25519 seeds as the
sole out-of-band credential.

=== Summary

#figure(
  placement: none,
  table(
    columns: (1.2fr, 1.5fr, 1.3fr, 1fr),
    align: (left, left, left, left),
    [*Approach*], [*Threats addressed*], [*Overhead*], [*Infrastructure required*],
    [IEEE 1609.2 @ieee-1609-2],
      [Heartbeat/control forgery; broadcast integrity; Sybil (via PKI)],
      [ECDSA-P256 per message; certificate transmission ($approx$250 B/msg)],
      [Full PKI hierarchy; CRL distribution; enrolment CA],
    [TESLA @tesla],
      [Broadcast message forgery (routing poisoning)],
      [HMAC per message; hash-chain bootstrapping; 1-period auth latency],
      [GPS time synchronisation; hash chain bootstrapping handshake],
    [CONFIDANT/watchdog @confidant @watchdog],
      [Black hole / grey hole; routing misbehavior],
      [Promiscuous mode monitoring; reputation messaging],
      [None (local detection); reputation gossip adds control traffic],
    [This work: KEM/DH + AEAD + Ed25519/ML-DSA-65],
      [Payload eavesdropping; payload tampering by relay; MitM on key exchange; quantum adversaries (with ML-KEM-768 + ML-DSA-65)],
      [2-message handshake at session setup; 28 B per encrypted frame; optional signing extension (Ed25519: 101 B; ML-DSA-65: 5266 B)],
      [None (no signatures) or pre-provisioned 32-byte seeds per node pair (PKI mode); larger frames with ML-KEM-768/ML-DSA-65],
  ),
  caption: [Comparison of VANET security approaches],
) <tab-security-comparison>

== Attack Surface Coverage <sec-attack-coverage>

@tab-attack-coverage maps each attack class from the threat taxonomy
(introduced in @background) to the mechanism(s) in vigilant-parakeet
that address it, and identifies those that remain as open items.

#figure(
  placement: none,
  table(
    columns: (1fr, auto, 1fr),
    align: (left, center, left),
    [*Attack*], [*Addressed?*], [*Mechanism / open item*],
    [Passive payload eavesdropping],
      [Yes],
      [AEAD encryption (AES-256-GCM / ChaCha20-Poly1305). Session key never held by relay nodes.],
    [Active payload tampering by relay],
      [Yes],
      [AEAD authentication tag. Decryption fails immediately if the tag does not verify; the tampered frame is dropped.],
    [Man-in-the-middle on key exchange (classical)],
      [Partial],
      [Addressed when signatures are enabled (Ed25519/ML-DSA-65). Without signatures, an active MitM can substitute key material undetected.],
    [Man-in-the-middle with quantum computer],
      [Yes, with PQ config],
      [ML-KEM-768 + ML-DSA-65 provides quantum-resistant KEM and authentication. X25519/Ed25519 remain vulnerable to Shor's algorithm.],
    [Harvest-now-decrypt-later (HNDL)],
      [Yes, with PQ config],
      [Session keys derived via HKDF from ML-KEM-768 KEM output; cannot be decrypted retroactively without the OBU's decapsulation key.],
    [HeartbeatReply replay (route freshness)],
      [Yes],
      [`ReplayWindow` per-sender sliding bitmask at RSU. Replayed replies outside the 64-sequence window are silently dropped.],
    [Replay window poisoning (forged large seq)],
      [Yes],
      [RSU validates sequence number against sent-heartbeat history before updating the replay window. Forged IDs outside the sent history are rejected.],
    [Session revocation replay],
      [Yes],
      [`SessionTerminated` carries timestamp (60 s validity) + 8-byte nonce. Time-bounded nonce cache at the OBU rejects replays within the validity window.],
    [Routing table poisoning (injected heartbeats)],
      [Partial],
      [Replay of old heartbeats is blocked by `ReplayWindow`. *Injection* of crafted fresh-looking heartbeats with arbitrary sequence numbers is not cryptographically prevented — heartbeat authentication (HMAC/TESLA) is future work.],
    [Black hole / grey hole attacks],
      [No],
      [Watchdog monitoring and reputation systems are not implemented. Identified as future work.],
    [Sybil attacks],
      [Partial],
      [In PKI mode, each OBU VANET MAC must match a pre-registered verifying key; an attacker cannot present an arbitrary new identity. TOFU mode does not prevent Sybil attacks at first contact.],
    [Wormhole attacks],
      [No],
      [Packet leashes or timing-based detection are not implemented. The latency-aware routing metric may mitigate (an artificial shortcut that inflates RTTs would score poorly), but provides no cryptographic guarantee.],
  ),
  caption: [Attack surface coverage summary],
) <tab-attack-coverage>

The table reveals a consistent pattern: the implemented security mechanisms
address the *data-plane* threats (payload confidentiality and integrity) and
the *key exchange* threats (MitM, replay, quantum adversaries) comprehensively,
while leaving *control-plane authentication* (heartbeat injection) as the primary
remaining gap. This gap is acknowledged in @l3-security-vehicular and is the
most impactful direction for future work: authenticating heartbeat messages would
close the routing table poisoning attack surface while preserving the existing
data-plane security architecture unchanged.
