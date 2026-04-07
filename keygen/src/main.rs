//! Signing keypair generator and node config certificate updater.
//!
//! Supports both Ed25519 (classical) and ML-DSA-65 (quantum-resistant) algorithms.
//!
//! # Subcommands
//!
//! ## generate
//! Generates a keypair and prints the seed and verifying key to stdout.
//!
//! ```text
//! keygen generate [ed25519|ml-dsa-65]
//! ```
//!
//! ## update-certs
//! Reads a list of node YAML config files and patches signing key fields in-place.
//! Use `--server-key`, `--client-keys`, or omit both to update everything.
//!
//! The signing algorithm is read from each config's `signing_algorithm` field.
//! Pass `--algo` to override all nodes with the same algorithm.
//!
//! ```text
//! keygen update-certs --server server.yaml --client obu1.yaml --client obu2.yaml
//! keygen update-certs --server server.yaml --client obu1.yaml --server-key
//! keygen update-certs --server server.yaml --client obu1.yaml --client-keys
//! keygen update-certs --server server.yaml --client obu1.yaml --algo ml-dsa-65
//! ```
//!
//! Fields written:
//! - Server config: `signing_key_seed`, `dh_signing_allowlist` (keyed by `vanet_mac`)
//! - Client config: `signing_key_seed`, `server_signing_pubkey`

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use node_lib::crypto::{SigningAlgorithm, SigningKeypair};
use std::path::{Path, PathBuf};

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Generate signing keypairs and update node config certificates"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a signing keypair and print the seed and verifying key
    Generate {
        /// Signing algorithm: ed25519 (default) or ml-dsa-65
        #[arg(default_value = "ed25519")]
        algo: String,
    },
    /// Update signing keys in node YAML config files in-place
    UpdateCerts(UpdateCertsArgs),
}

#[derive(Args)]
struct UpdateCertsArgs {
    /// Path to the server YAML config file
    #[arg(long)]
    server: Option<PathBuf>,

    /// Path(s) to OBU/client YAML config files (repeat for multiple)
    #[arg(long = "client", value_name = "CLIENT_YAML", num_args = 1..)]
    clients: Vec<PathBuf>,

    /// Override signing algorithm for all nodes: ed25519 or ml-dsa-65.
    /// When omitted, the algorithm is read from each config's `signing_algorithm` field,
    /// falling back to ed25519 if not present.
    #[arg(long)]
    algo: Option<String>,

    /// Update the server signing key only
    /// (propagates server verifying key to all client configs as `server_signing_pubkey`)
    #[arg(long)]
    server_key: bool,

    /// Update client signing keys only
    /// (propagates each client verifying key to the server's `dh_signing_allowlist`)
    #[arg(long)]
    client_keys: bool,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Generate { algo } => cmd_generate(&algo),
        Commands::UpdateCerts(args) => cmd_update_certs(args),
    }
}

// ── generate ─────────────────────────────────────────────────────────────────

fn cmd_generate(algo_str: &str) -> Result<()> {
    let algo = parse_algo(algo_str)?;
    let kp = SigningKeypair::generate(algo);
    let seed_hex = hex_encode(kp.seed_bytes());
    let pubkey_hex = hex_encode(kp.verifying_key_bytes());

    println!("{} signing keypair for DH authentication", algo_label(algo));
    println!();
    println!("Seed (signing_key_seed in node YAML — keep secret):");
    println!("  {seed_hex}");
    println!();
    println!(
        "Verifying key (for dh_signing_allowlist on server, or server_signing_pubkey on OBU):"
    );
    println!("  {pubkey_hex}");
    Ok(())
}

// ── update-certs ──────────────────────────────────────────────────────────────

fn cmd_update_certs(args: UpdateCertsArgs) -> Result<()> {
    // Validate the --algo override early so we fail fast before touching any files.
    if let Some(ref s) = args.algo {
        parse_algo(s)?;
    }

    // If neither flag is given, update everything.
    let update_server_key = args.server_key || !args.client_keys;
    let update_client_keys = args.client_keys || !args.server_key;

    // Validate inputs.
    if update_server_key && args.server.is_none() {
        bail!("--server is required when updating the server key (--server-key or default both)");
    }
    if update_client_keys && args.clients.is_empty() {
        bail!(
            "at least one --client is required when updating client keys (--client-keys or default both)"
        );
    }

    // ── 1. Generate server keypair (if requested) ─────────────────────────

    let server_pubkey_hex: Option<String> = if update_server_key {
        let server_path = args.server.as_ref().unwrap();
        let mut yaml = read_yaml(server_path)?;

        // Use --algo override, or read signing_algorithm from config, or default to ed25519.
        let algo = resolve_algo(&yaml, args.algo.as_deref())?;

        let kp = SigningKeypair::generate(algo);
        let seed_hex = hex_encode(kp.seed_bytes());
        let pubkey_hex = hex_encode(kp.verifying_key_bytes());
        println!("[server] generated {} keypair", algo_label(algo));
        println!("  signing_key_seed  = {seed_hex}");
        println!("  verifying_key     = {pubkey_hex}");

        set_str(&mut yaml, "signing_key_seed", &seed_hex);
        set_str(&mut yaml, "signing_algorithm", algo_config_name(algo));
        write_yaml(server_path, &yaml)?;
        println!("  → wrote {}", server_path.display());
        Some(pubkey_hex)
    } else {
        None
    };

    if args.clients.is_empty() {
        return Ok(());
    }

    // ── 2. Process each client config ────────────────────────────────────

    // We accumulate (vanet_mac, pubkey_hex) pairs to patch the server allowlist.
    let mut allowlist_updates: Vec<(String, String)> = Vec::new();

    for client_path in &args.clients {
        let name = client_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| client_path.display().to_string());

        let mut yaml =
            read_yaml(client_path).with_context(|| format!("reading {}", client_path.display()))?;

        // Propagate server verifying key → server_signing_pubkey in client config.
        if let Some(ref pubkey) = server_pubkey_hex {
            set_str(&mut yaml, "server_signing_pubkey", pubkey);
        }

        if update_client_keys {
            // Use --algo override, or read signing_algorithm from this client's config,
            // or default to ed25519.
            let algo = resolve_algo(&yaml, args.algo.as_deref())?;

            let kp = SigningKeypair::generate(algo);
            let seed_hex = hex_encode(kp.seed_bytes());
            let pubkey_hex = hex_encode(kp.verifying_key_bytes());

            set_str(&mut yaml, "signing_key_seed", &seed_hex);
            set_str(&mut yaml, "signing_algorithm", algo_config_name(algo));

            println!("[client {name}] generated {} keypair", algo_label(algo));
            println!("  signing_key_seed  = {seed_hex}");
            println!("  verifying_key     = {pubkey_hex}");

            // Collect vanet_mac for server allowlist update.
            match yaml
                .get("vanet_mac")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            {
                Some(mac) => {
                    allowlist_updates.push((mac, pubkey_hex));
                }
                None => {
                    println!(
                        "  WARNING: no vanet_mac in {name} — cannot add to server dh_signing_allowlist"
                    );
                }
            }
        }

        write_yaml(client_path, &yaml)
            .with_context(|| format!("writing {}", client_path.display()))?;
        println!("  → wrote {}", client_path.display());
    }

    // ── 3. Patch server dh_signing_allowlist (if we generated client keys) ──

    if update_client_keys && !allowlist_updates.is_empty() {
        if let Some(ref server_path) = args.server {
            let mut yaml = read_yaml(server_path)
                .with_context(|| format!("re-reading {}", server_path.display()))?;

            // Retrieve existing allowlist mapping or start a new one.
            let mut allowlist = yaml
                .get("dh_signing_allowlist")
                .and_then(|v| v.as_mapping().cloned())
                .unwrap_or_default();

            for (mac, pubkey) in &allowlist_updates {
                allowlist.insert(
                    serde_yaml::Value::String(mac.clone()),
                    serde_yaml::Value::String(pubkey.clone()),
                );
                println!("[server] dh_signing_allowlist[{mac}] = {pubkey}");
            }

            yaml["dh_signing_allowlist"] = serde_yaml::Value::Mapping(allowlist);
            write_yaml(server_path, &yaml)
                .with_context(|| format!("writing {}", server_path.display()))?;
            println!("  → wrote {} (allowlist updated)", server_path.display());
        } else {
            println!(
                "NOTE: no --server provided; skipping dh_signing_allowlist update. \
                 Add these entries manually:"
            );
            for (mac, pubkey) in &allowlist_updates {
                println!("  {mac}: {pubkey}");
            }
        }
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_algo(s: &str) -> Result<SigningAlgorithm> {
    match s.to_lowercase().as_str() {
        "ed25519" => Ok(SigningAlgorithm::Ed25519),
        "ml-dsa-65" | "mldsa65" | "dilithium3" => Ok(SigningAlgorithm::MlDsa65),
        other => bail!("unknown algorithm '{other}'; supported: ed25519, ml-dsa-65"),
    }
}

/// Resolve which signing algorithm to use for a given node config.
///
/// Priority (highest first):
/// 1. `--algo` CLI override
/// 2. `signing_algorithm` field in the YAML config
/// 3. `ed25519` (default)
fn resolve_algo(yaml: &serde_yaml::Value, override_algo: Option<&str>) -> Result<SigningAlgorithm> {
    if let Some(s) = override_algo {
        return parse_algo(s);
    }
    if let Some(s) = yaml.get("signing_algorithm").and_then(|v| v.as_str()) {
        return parse_algo(s);
    }
    Ok(SigningAlgorithm::Ed25519)
}

fn algo_label(algo: SigningAlgorithm) -> &'static str {
    match algo {
        SigningAlgorithm::Ed25519 => "Ed25519",
        SigningAlgorithm::MlDsa65 => "ML-DSA-65",
    }
}

/// The canonical string written to the `signing_algorithm` YAML field.
/// Must match what the node config parser accepts.
fn algo_config_name(algo: SigningAlgorithm) -> &'static str {
    match algo {
        SigningAlgorithm::Ed25519 => "ed25519",
        SigningAlgorithm::MlDsa65 => "ml-dsa-65",
    }
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn read_yaml(path: &Path) -> Result<serde_yaml::Value> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Empty file → start with an empty mapping.
    if content.trim().is_empty() {
        return Ok(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }
    serde_yaml::from_str(&content).with_context(|| format!("parsing YAML in {}", path.display()))
}

fn write_yaml(path: &Path, value: &serde_yaml::Value) -> Result<()> {
    let out = serde_yaml::to_string(value)
        .with_context(|| format!("serialising YAML for {}", path.display()))?;
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// Insert or overwrite a string key in a YAML mapping.
///
/// Promotes a non-mapping root to a mapping (should not happen with valid node configs).
fn set_str(yaml: &mut serde_yaml::Value, key: &str, value: &str) {
    if !yaml.is_mapping() {
        *yaml = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    yaml[key] = serde_yaml::Value::String(value.to_string());
}
