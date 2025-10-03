//! Topology configuration parsing for simulator.
//!
//! This module handles reading and parsing the simulator configuration file,
//! extracting node definitions and network topology connections with their
//! channel parameters (latency, jitter, packet loss).

use anyhow::Result;
use common::channel_parameters::ChannelParameters;
use config::{Config, Value};
use std::collections::HashMap;

/// Parsed topology configuration containing node definitions and connections.
pub struct TopologyConfig {
    /// Node definitions: node_name -> config parameters
    pub nodes: HashMap<String, HashMap<String, Value>>,
    /// Topology connections: from_node -> to_node -> channel parameters
    pub connections: HashMap<String, HashMap<String, ChannelParameters>>,
}

impl TopologyConfig {
    /// Parse topology from configuration file.
    ///
    /// Reads the YAML configuration file and extracts:
    /// - Node definitions from the "nodes" table
    /// - Network topology connections from the "topology" table
    ///
    /// # Arguments
    /// * `config_file` - Path to the YAML configuration file
    ///
    /// # Returns
    /// Parsed topology configuration ready for device and channel creation.
    pub fn from_file(config_file: &str) -> Result<Self> {
        let config = Self::load_config(config_file)?;
        let nodes = Self::parse_nodes(&config)?;
        let connections = Self::parse_topology_connections(&config)?;

        Ok(Self { nodes, connections })
    }

    /// Load configuration file.
    fn load_config(config_file: &str) -> Result<Config> {
        Config::builder()
            .add_source(config::File::with_name(config_file))
            .build()
            .map_err(Into::into)
    }

    /// Parse node definitions from configuration.
    ///
    /// Extracts the "nodes" table from the config, where each entry is:
    /// node_name -> { config_path: "path/to/node.yaml", ... }
    fn parse_nodes(config: &Config) -> Result<HashMap<String, HashMap<String, Value>>> {
        let nodes = config
            .get_table("nodes")?
            .iter()
            .filter_map(|(key, val)| {
                val.clone()
                    .into_table()
                    .ok()
                    .map(|table| (key.clone(), table))
            })
            .collect();
        Ok(nodes)
    }

    /// Parse topology connections from configuration.
    ///
    /// Extracts the "topology" table from the config, where each entry defines
    /// connections between nodes with channel parameters:
    /// from_node -> to_node -> { latency: 10, jitter: 5, loss: 0.01 }
    fn parse_topology_connections(
        config: &Config,
    ) -> Result<HashMap<String, HashMap<String, ChannelParameters>>> {
        let topology = config
            .get_table("topology")?
            .iter()
            .filter_map(|(key, val)| {
                val.clone().into_table().ok().map(|v| {
                    (
                        key.clone(),
                        v.iter()
                            .map(|(onode, param)| {
                                let param = param.clone().into_table().unwrap_or_default();
                                let param = ChannelParameters::from(param);
                                (onode.clone(), param)
                            })
                            .collect(),
                    )
                })
            })
            .collect();
        Ok(topology)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config_nonexistent() {
        let result = TopologyConfig::load_config("nonexistent.yaml");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_nodes_invalid_format() {
        let config = Config::builder()
            .set_override("nodes", "not a table")
            .unwrap()
            .build()
            .unwrap();

        let result = TopologyConfig::parse_nodes(&config);
        assert!(result.is_err());
    }
}
