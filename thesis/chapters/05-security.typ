// ── Chapter 5 — Security Architecture <security> ─────────────────────────────

= Security Architecture <security>

A key motivation for this project is the study of Layer-3 security in
vehicular networks @l3-security-vehicular. vigilant-parakeet therefore
implements a full end-to-end security layer enabling OBUs to establish
encrypted sessions with a backend server without trusting intermediate
RSUs or relay OBUs.

== Threat Model

The security model assumes:

- *Untrusted relay nodes*: intermediate OBUs and even RSUs may be
  compromised. They must not be able to read or tamper with payload data
  exchanged between a legitimate OBU and the server.

- *Passive eavesdroppers*: an attacker can capture all frames on any link
  in the network.

- *No PKI requirement at session setup*: to keep the protocol lightweight,
  key exchange uses ephemeral Diffie-Hellman rather than certificate-based
  authentication. Mutual authentication can be layered on top.

The design consequence is *end-to-end encryption at the OBU–server boundary*:
RSUs and relay OBUs forward encrypted payloads opaquely and never hold the
session key.

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

Two new control message types are defined in
`node_lib::messages::control::key_exchange`:

#figure(
  table(
    columns: (auto, auto, 1fr),
    align: (left, left, left),
    [*Field*], [*Size*], [*Description*],
    [`key_id`], [4 B], [Monotonically increasing exchange identifier, used as HKDF salt input to bind the derived key to this round.],
    [`public_key`], [32 B], [X25519 ephemeral public key of the sender.],
    [`sender`], [6 B], [MAC address of the originating node.],
  ),
  caption: [`KeyExchangeInit` and `KeyExchangeReply` wire format (42 bytes each)],
) <tab-ke-wire>

Both `KeyExchangeInit` and `KeyExchangeReply` share this layout. The
distinguishing information is the message type discriminant in the outer
`Message` container.

=== Handshake Flow

The handshake proceeds as follows (@fig-dh-handshake):

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
  ```
  OBU                  RSU (relay)           Server
   │                       │                    │
   │── KeyExchangeInit ───►│                    │
   │   (key_id, pub_A)     │── KeyExchangeInit ►│
   │                       │   (forwarded)      │ generate pub_B
   │                       │                    │ shared = DH(priv_B, pub_A)
   │                       │                    │ key = HKDF(shared, key_id)
   │                       │◄─ KeyExchangeReply ─│
   │◄─ KeyExchangeReply ───│   (key_id, pub_B)  │
   │   (key_id, pub_B)     │   (forwarded)      │
   │ shared = DH(priv_A, pub_B)                 │
   │ key = HKDF(shared, key_id)                 │
   │                       │                    │
   │═══════════ Encrypted data ════════════════►│
  ```,
  caption: [X25519 Diffie-Hellman handshake and subsequent encrypted data flow],
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

```
┌──────────────┬───────────────────────────────────┬──────────────────┐
│  nonce (12 B)│     ciphertext (variable)          │  auth tag (16 B) │
└──────────────┴───────────────────────────────────┴──────────────────┘
                         total overhead: 28 bytes
```

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
  ```
  ┌─────────┐  initiate_exchange()  ┌─────────┐
  │  None   │──────────────────────►│ Pending │
  └─────────┘                       └────┬────┘
       ▲                                 │ complete_exchange()
       │  remove_pending()               │ (key_id match)
       │  (max retries)                  ▼
       │                           ┌─────────────┐
       └──────────────────────────-│ Established │
         is_key_expired() → true   └─────────────┘
  ```,
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

== RSU Opacity

A deliberate design property is that RSUs handle encrypted `Data` frames
*opaquely*: the RSU forwards `ToUpstream` payloads to the server
without attempting to decrypt them. This is validated by the RSU
encryption test suite, which confirms that even invalid ciphertext is
forwarded unchanged. The effect is that compromising an RSU does not
expose session keys or allow payload manipulation; only the endpoints
hold the HKDF-derived key.
