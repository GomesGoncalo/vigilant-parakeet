use clap::Parser;
use node_lib::crypto::{DhGroup, KdfAlgorithm, SymmetricCipher};
use std::net::Ipv4Addr;

#[derive(clap::Args, Clone, Debug)]
pub struct ObuParameters {
    /// Hello history
    #[arg(long, default_value_t = 10)]
    pub hello_history: u32,

    /// Number of cached upstream candidates to keep for fast failover
    #[arg(long, default_value_t = 3)]
    pub cached_candidates: u32,

    /// Enable payload encryption (implies DH key exchange with server)
    #[arg(long, default_value_t = false)]
    pub enable_encryption: bool,

    /// Sign DH key exchange messages with Ed25519 and verify peer signatures.
    /// When enabled, each node generates a random identity keypair at startup.
    /// Both sides must have this enabled for signatures to be verified.
    #[arg(long, default_value_t = false)]
    pub enable_dh_signatures: bool,

    /// Hex-encoded 32-byte Ed25519 seed for a stable signing identity (64 hex chars).
    /// When set alongside enable_dh_signatures, this node uses the derived keypair
    /// instead of generating a random one at startup.  The corresponding public key
    /// can be pre-registered in the server's dh_signing_allowlist for PKI-mode auth.
    /// Not exposed as a CLI flag to avoid leaking seeds via process listings.
    #[arg(skip)]
    pub signing_key_seed: Option<String>,

    /// Interval in milliseconds between DH re-key exchanges
    #[arg(long, default_value_t = 43_200_000)]
    pub dh_rekey_interval_ms: u64,

    /// Maximum lifetime of a DH-derived key in milliseconds before forced re-key
    #[arg(long, default_value_t = 86_400_000)]
    pub dh_key_lifetime_ms: u64,

    /// Timeout in milliseconds to wait for a DH reply before retrying
    #[arg(long, default_value_t = 5_000)]
    pub dh_reply_timeout_ms: u64,

    /// Symmetric cipher: aes-256-gcm, aes-128-gcm, chacha20-poly1305
    #[arg(long, default_value_t = SymmetricCipher::default())]
    pub cipher: SymmetricCipher,

    /// Key derivation function: hkdf-sha256, hkdf-sha384, hkdf-sha512
    #[arg(long, default_value_t = KdfAlgorithm::default())]
    pub kdf: KdfAlgorithm,

    /// DH group for key exchange: x25519
    #[arg(long, default_value_t = DhGroup::default())]
    pub dh_group: DhGroup,
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct ObuArgs {
    /// Interface to bind to
    #[arg(short, long)]
    pub bind: String,

    /// Virtual device name
    #[arg(short, long)]
    pub tap_name: Option<String>,

    /// IP
    #[arg(short, long)]
    pub ip: Option<Ipv4Addr>,

    /// MTU
    #[arg(short, long, default_value_t = 1400)]
    pub mtu: i32,

    /// OBU Parameters
    #[command(flatten)]
    pub obu_params: ObuParameters,
}
