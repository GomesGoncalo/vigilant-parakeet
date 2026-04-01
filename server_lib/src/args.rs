use clap::Parser;
use node_lib::crypto::{DhGroup, KdfAlgorithm, SymmetricCipher};
use std::net::Ipv4Addr;

#[derive(clap::Args, Clone, Debug)]
pub struct ServerParameters {
    /// UDP port to listen on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Enable encryption for OBU traffic (implies DH key exchange)
    #[arg(long, default_value_t = false)]
    pub enable_encryption: bool,

    /// Sign DH key exchange replies with Ed25519 and verify incoming signatures.
    /// Must be enabled on all participating nodes for end-to-end signature checking.
    #[arg(long, default_value_t = false)]
    pub enable_dh_signatures: bool,

    /// Maximum lifetime of a DH-derived key in milliseconds before the server
    /// considers it expired and drops traffic until the OBU re-keys
    #[arg(long, default_value_t = 86_400_000)]
    pub key_ttl_ms: u64,

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
pub struct ServerArgs {
    /// IP address to bind to
    #[arg(short, long)]
    pub ip: Ipv4Addr,

    /// Server Parameters
    #[command(flatten)]
    pub server_params: ServerParameters,
}
