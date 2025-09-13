use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,

    #[arg(short, long, default_value_t = false)]
    pub pretty: bool,
}
