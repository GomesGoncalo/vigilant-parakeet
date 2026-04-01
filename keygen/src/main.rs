//! Generate Ed25519 signing keypairs for DH message authentication.
//!
//! Prints the seed (for `signing_key_seed` in node YAML) and the verifying
//! key (for `dh_signing_allowlist` on the server, or `server_signing_pubkey`
//! on an OBU).  Uses a cryptographically secure RNG (`OsRng`).
//!
//! Usage:
//!   keygen

fn main() {
    let kp = node_lib::crypto::SigningKeypair::generate();
    let seed_hex: String = kp.seed_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let pubkey_hex: String = kp
        .verifying_key_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    println!("Ed25519 signing keypair for DH authentication");
    println!();
    println!("Seed (signing_key_seed in node YAML — keep secret):");
    println!("  {seed_hex}");
    println!();
    println!(
        "Verifying key (for dh_signing_allowlist on server, or server_signing_pubkey on OBU):"
    );
    println!("  {pubkey_hex}");
}
