// Tab-specific rendering modules
mod channels;
mod logs;
mod metrics;
mod topology;
mod upstreams;

pub use channels::render_channels_tab;
pub use logs::render_logs_tab;
pub use metrics::render_metrics_tab;
pub use topology::render_topology_tab;
pub use upstreams::render_upstreams_tab;
