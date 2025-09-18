use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod autofix_configs;
mod build_summary;
mod generate_pairs;
mod merge_latency;
mod ns_addrs;
mod parse_band;
mod validate_configs;

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// parse iperf json and print Mbits/s
    #[command(alias = "parseband")]
    ParseBand { json: PathBuf },
    /// build json summary from csv
    #[command(alias = "buildsummary")]
    BuildSummary {
        out_csv: PathBuf,
        json_out: PathBuf,
        summary_csv: PathBuf,
    },
    /// merge measured latency into iperf json
    #[command(alias = "mergelatency")]
    MergeLatency { ipf: PathBuf },
    /// convert comma-separated addrs from stdin to JSON array
    #[command(alias = "nsaddrs")]
    NsAddrs,
    /// generate OBU->RSU pairs from simulator node_info JSON
    #[command(alias = "generatepairs")]
    GeneratePairs {
        json: std::path::PathBuf,
        time: String,
        repeat: String,
    },
    /// validate YAML config files (basic checks)
    #[command(alias = "validateconfigs")]
    ValidateConfigs { files: Vec<std::path::PathBuf> },
    /// auto-fix simple YAML issues (backups; use --dry-run first)
    #[command(alias = "autofixconfigs")]
    AutoFixConfigs {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
        /// allocate missing IPs inside this CIDR (e.g. 10.0.0.0/24)
        #[arg(long)]
        ip_cidr: Option<String>,
        /// default latency to set when missing
        #[arg(long, default_value_t = 10)]
        default_latency: i64,
        /// default loss to set when missing
        #[arg(long, default_value_t = 0)]
        default_loss: i64,
        /// scan examples/*.yaml when --all is set
        #[arg(long)]
        all: bool,
        /// path to allocation DB (JSON) to persist assignments
        #[arg(long)]
        alloc_db: Option<String>,
        /// start offset for allocation (last octet minimum, e.g. 10)
        #[arg(long, default_value_t = 10u8)]
        start_offset: u8,
        files: Vec<std::path::PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::ParseBand { json } => parse_band::run(json)?,
        Commands::BuildSummary {
            out_csv,
            json_out,
            summary_csv,
        } => build_summary::run(out_csv, json_out, summary_csv)?,
        Commands::MergeLatency { ipf } => merge_latency::run(ipf)?,
        Commands::NsAddrs => ns_addrs::run()?,
        Commands::GeneratePairs { json, time, repeat } => generate_pairs::run(json, time, repeat)?,
        Commands::ValidateConfigs { files } => validate_configs::run(files)?,
        Commands::AutoFixConfigs {
            dry_run,
            backup,
            ip_cidr,
            default_latency,
            default_loss,
            all,
            alloc_db,
            start_offset,
            files,
        } => {
            let mut paths = files;
            if all {
                // discover examples/*.yaml
                let mut ex = Vec::new();
                for p in (glob::glob("examples/*.yaml")?).flatten() {
                    ex.push(p);
                }
                paths.extend(ex);
            }
            autofix_configs::run(paths.clone(), dry_run, backup)?;
            // apply allocator if cidr provided or if AUTOALLOC tokens exist
            let alloc_opts = autofix_configs::AllocOptions {
                cidr: ip_cidr,
                default_latency,
                default_loss,
                dry_run,
                backup,
                alloc_db,
                start_offset,
            };
            autofix_configs::apply_allocator(paths, alloc_opts)?;
        }
    }
    Ok(())
}
