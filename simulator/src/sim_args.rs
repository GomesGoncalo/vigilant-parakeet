use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,

    /// Pretty print logs
    #[arg(short, long, default_value_t = false)]
    pub pretty: bool,

    /// Enable TUI (Terminal User Interface) dashboard
    #[arg(short, long, default_value_t = false)]
    pub tui: bool,
}
