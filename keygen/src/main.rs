//! Generate signing keypairs for DH message authentication.
//!
//! Supports both Ed25519 (classical) and ML-DSA-65 (quantum-resistant) algorithms.
//! Prints the seed (for `signing_key_seed` in node YAML) and the verifying key
//! (for `dh_signing_allowlist` on the server, or `server_signing_pubkey` on an OBU).
//!
//! Usage:
//!   keygen [ed25519|ml-dsa-65]   (default: ed25519)

fn main() {
    let algo_str = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ed25519".to_string());
    let algo = match algo_str.to_lowercase().as_str() {
        "ed25519" => node_lib::crypto::SigningAlgorithm::Ed25519,
        "ml-dsa-65" | "mldsa65" | "dilithium3" => node_lib::crypto::SigningAlgorithm::MlDsa65,
        other => {
            eprintln!("Unknown algorithm: {other}");
            eprintln!("Supported: ed25519, ml-dsa-65");
            std::process::exit(1);
        }
    };

    let kp = node_lib::crypto::SigningKeypair::generate(algo);
    let seed_hex: String = kp.seed_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let pubkey_hex: String = kp
        .verifying_key_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let algo_name = match algo {
        node_lib::crypto::SigningAlgorithm::Ed25519 => "Ed25519",
        node_lib::crypto::SigningAlgorithm::MlDsa65 => "ML-DSA-65",
    };

    println!("{algo_name} signing keypair for DH authentication");
    println!();
    println!("Seed (signing_key_seed in node YAML — keep secret):");
    println!("  {seed_hex}");
    println!();
    println!(
        "Verifying key (for dh_signing_allowlist on server, or server_signing_pubkey on OBU):"
    );
    println!("  {pubkey_hex}");
}
