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
    pub cipher:   SymmetricCipher,  // default: AES-256-GCM
    pub kdf:      KdfAlgorithm,     // default: HKDF-SHA256
    pub dh_group: DhGroup,          // default: X25519
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
      [`x25519` (default)],
      [X25519 (Curve25519 @x25519) provides 128-bit security with fast, constant-time scalar multiplication.],
  ),
  caption: [Configurable cipher suite parameters],
) <tab-cipher-suite>

== Key Exchange Protocol

Key exchange follows a two-message Diffie-Hellman handshake carried over
the existing control-plane message infrastructure.

=== Wire Format

Two control message types are defined in
`node_lib::messages::control::key_exchange`. Both `KeyExchangeInit` and
`KeyExchangeReply` share the same layout; the distinguishing information is
the message type discriminant in the outer `Message` container.

*Unsigned format (42 bytes)*:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Field*], [*Size*], [*Description*],
    [`key_id`], [4 B], [Monotonically increasing exchange identifier, used as HKDF salt input to bind the derived key to this round.],
    [`public_key`], [32 B], [X25519 ephemeral public key of the sender.],
    [`sender`], [6 B], [MAC address of the originating node.],
  ),
  caption: [Unsigned `KeyExchangeInit` / `KeyExchangeReply` base format (42 bytes)],
) <tab-ke-wire>

*Signed format (138 bytes)*: when Ed25519 authentication is enabled (see
@sec-dh-auth), a 96-byte extension is appended to the base payload:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Field*], [*Size*], [*Description*],
    [`key_id`],         [4 B],  [Same as base format.],
    [`public_key`],     [32 B], [Same as base format.],
    [`sender`],         [6 B],  [Same as base format.],
    [`signing_pubkey`], [32 B], [Ed25519 verifying key of the sender. Present only in signed messages.],
    [`signature`],      [64 B], [Ed25519 signature over the first 42 bytes (base payload). Present only in signed messages.],
  ),
  caption: [Signed `KeyExchangeInit` / `KeyExchangeReply` format (138 bytes)],
) <tab-ke-signed-wire>

The `TryFrom<&[u8]>` deserialiser accepts both lengths: a 42-byte buffer
produces an unsigned message; a ≥138-byte buffer triggers parsing of the
signature extension. Buffers between 42 and 137 bytes are rejected as
malformed. The cloud-protocol `KeyExchangeForward` (type `0x04`) and
`KeyExchangeResponse` (type `0x05`) messages were updated to accept both
payload lengths, preserving backward compatibility with unsigned deployments.

=== Handshake Flow

The handshake proceeds as follows (see figure):

+ The OBU calls `DhKeyStore::initiate_exchange(server_mac)`, which
  generates a new `DhKeypair`, assigns the next `key_id`, and stores a
  `PendingExchange` entry with a timestamp.

+ The OBU sends a `KeyExchangeInit` message toward the server via its
  upstream route. Intermediate OBUs and the RSU *forward* the message
  without modification. The RSU wraps it in a `KeyExchangeForward` UDP
  message (type `0x04`) and sends it to the server over the cloud interface.

+ The server's `DhKeyStore` calls `handle_incoming_init`, generates its
  own ephemeral keypair, computes the X25519 shared secret, derives the
  session key via HKDF, stores an `ObuKey` (keyed by OBU VANET MAC), and
  returns its own public key bytes.

+ The server wraps its `KeyExchangeReply` in a `KeyExchangeResponse` UDP
  message (type `0x05`) and sends it to the RSU, which delivers it on the
  VANET to the OBU.

+ When the OBU receives the reply, it calls
  `DhKeyStore::complete_exchange`, verifies the `key_id`, derives the
  session key, and moves the entry to `established`.

At this point both sides hold the same session key without any key
material having crossed the network in plaintext.

#figure(
  scale(70%, reflow: true, chronos.diagram({
    import chronos: *
    _par("OBU")
    _par("RSU", display-name: "RSU (relay)")
    _par("Server")
    _seq("OBU", "RSU", comment: "KeyExchangeInit")
    _seq("RSU", "Server", comment: "KeyExchangeForward")
    _seq("Server", "Server", comment: "gen pub_B; DH→key")
    _seq("Server", "RSU", comment: "KeyExchangeResponse")
    _seq("RSU", "OBU", comment: "KeyExchangeReply")
    _seq("OBU", "OBU", comment: "DH→key")
    _sep("Encrypted channel established")
    _seq("OBU", "Server", comment: "Encrypted data")
  }, width: 150mm)),
  caption: [X25519 Diffie-Hellman handshake (unsigned) and subsequent encrypted data flow],
) <fig-dh-handshake>

== Key Derivation

The raw X25519 output (32 bytes) is not used directly as a symmetric key.
Instead, it is passed through HKDF @hkdf:

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
key even if the same DH keypair were reused.

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
    node((2,0), [Pending], name: <pending>),
    node((2,2), [Established], name: <established>),
    edge(<none>, <pending>, "->", [initiate\_exchange()], label-side: center),
    edge(<pending>, <established>, "->", [complete\_exchange()\ (key\_id match)], label-side: right),
    edge(<established>, <none>, "->",
      [is\_key\_expired() → true\ or remove\_pending() (max retries)],
      bend: -40deg, label-side: left),
  ),
  caption: [DH key store state machine per peer],
) <fig-dh-state>

Key lifecycle features:

- *Timeout and retry*: if no `KeyExchangeReply` arrives within
  `dh_key_lifetime_ms`, `is_pending_timed_out()` returns true and the
  OBU calls `reinitiate_exchange()`, which increments the retry counter
  and generates a fresh keypair.

- *Key expiry*: `is_key_expired()` compares the age of an established key
  against `dh_key_lifetime_ms`. Expired keys trigger a new handshake,
  providing forward secrecy through periodic re-keying.

- *Key ID mismatch rejection*: `complete_exchange()` drops any reply
  whose `key_id` does not match the pending exchange, preventing
  replay or injection of stale replies.

== Ed25519 Authentication for Key Exchange <sec-dh-auth>

The unauthenticated DH handshake described above is vulnerable to an active
man-in-the-middle attack: an adversary positioned between the OBU and the
server can intercept `KeyExchangeInit`, substitute its own X25519 public key,
complete a separate DH exchange with the server, and relay decrypted/re-encrypted
traffic transparently. Neither endpoint can detect the substitution from the
key bytes alone.

vigilant-parakeet addresses this with an optional *Ed25519 digital signature
layer* @rfc8032 applied to both handshake messages. The signature is carried
in-band inside the `KeyExchangeInit` and `KeyExchangeReply` messages as the
138-byte signed format described in @tab-ke-signed-wire.

=== Signing Identity

Each participating node holds a long-lived Ed25519 identity keypair,
encapsulated in `SigningKeypair` (`node_lib::crypto`):

```rust
pub struct SigningKeypair {
    inner: ed25519_dalek::SigningKey,
}
```

The signing key serves as a stable node identity across sessions, analogous
to an SSH host key. Two construction modes are supported:

- *Ephemeral*: `SigningKeypair::generate()` draws 32 bytes from the OS CSPRNG
  and creates a fresh keypair. Convenient for testing; the identity is lost on
  restart.

- *Persistent*: `SigningKeypair::from_seed(seed: [u8; 32])` derives the same
  keypair deterministically from a fixed seed. The seed is stored in node YAML
  under `signing_key_seed` (64 hex characters). The verifying key is stable
  across restarts, enabling pre-registration in peer allowlists.

At startup a node with `enable_dh_signatures: true` logs its 32-byte verifying
(public) key in hexadecimal. A companion `keygen` binary is provided for
generating seed/verifying-key pairs at provisioning time:

```sh
$ keygen
seed=<64 hex chars>   verifying_key=<64 hex chars>
```

=== Signature Computation and Verification

The signature covers the *base payload* of the handshake message — the first
42 bytes comprising `(key_id, public_key, sender)` — binding the authentication
to the specific DH round without committing to the appended signature fields
themselves:

```rust
let base = ke_init.base_payload();  // [key_id (4B) | public_key (32B) | sender (6B)]
let sig  = signing_keypair.sign(&base);
```

Verification uses `verify_dh_signature(message, signing_pubkey_bytes, signature_bytes)`
from `node_lib::crypto`, which wraps `ed25519_dalek::VerifyingKey::verify`.
A failed verification (wrong key, tampered message, or corrupted bytes) returns
`NodeError::SignatureError` and causes the handshake message to be silently
dropped.

=== Authenticated Handshake Flow

#figure(
  center(scale(70%, reflow: true, chronos.diagram({
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
  }, width: 150mm))),
  caption: [Authenticated DH handshake with Ed25519 signatures and optional PKI enforcement],
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
pre-registered Ed25519 verifying key (32 bytes, 64 hex characters in YAML):

```yaml
enable_dh_signatures: true
signing_key_seed: "<64-hex-chars>"
dh_signing_allowlist:
  "AA:BB:CC:DD:EE:FF": "<64-hex-chars>"   # OBU n2 verifying key
  "11:22:33:44:55:66": "<64-hex-chars>"   # OBU n3 verifying key
```

Any `KeyExchangeInit` whose `signing_pubkey` does not exactly match the
pre-registered value for the source MAC is rejected, regardless of whether
the Ed25519 signature itself is well-formed. This closes the first-contact
impersonation gap at the cost of manual key provisioning.

Symmetrically, an OBU can pin the server's verifying key via
`server_signing_pubkey` in its YAML, rejecting any `KeyExchangeReply` from
a server whose signing identity does not match:

```yaml
enable_dh_signatures: true
signing_key_seed: "<64-hex-chars>"
server_signing_pubkey: "<64-hex-chars>"   # expected server verifying key
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
      [Vulnerable to active MitM on key exchange.],
    [TOFU (signed, no allowlist)],
      [Post-first-contact key substitution MitM],
      [First-contact impersonation possible.],
    [PKI (signed + allowlist)],
      [First-contact and all subsequent impersonation],
      [Requires out-of-band key distribution; static allowlist has no revocation.],
  ),
  caption: [Security properties under each authentication configuration],
) <tab-auth-modes>

== RSU Opacity

A deliberate design property is that RSUs handle encrypted `Data` frames
*opaquely*: the RSU forwards `ToUpstream` payloads to the server
without attempting to decrypt them. This is validated by the RSU
encryption test suite, which confirms that even invalid ciphertext is
forwarded unchanged. The effect is that compromising an RSU does not
expose session keys or allow payload manipulation; only the endpoints
hold the HKDF-derived key.
