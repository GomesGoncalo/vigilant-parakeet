use crate::channel::Channel;
use crate::namespace::{NamespaceManager, NamespaceWrapper};
use crate::sim_args::SimArgs;
use anyhow::{Error, Result};
use common::device::Device;
use common::network_interface::NetworkInterface;
#[cfg(feature = "webview")]
use common::tun::Tun;
use config::Value;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use node_lib::{Node, PACKET_BUFFER_SIZE};
use server_lib::Server;
use std::{collections::HashMap, sync::Arc};

#[cfg(test)]
mod simulator_tests {
    use super::*;
    use crate::channel::Channel;
    use common::channel_parameters::ChannelParameters;
    use mac_address::MacAddress;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_channel_reads_returns_packet() {
        let (tun_a, tun_b) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);

        let params = ChannelParameters::from(HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);
        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // send data from peer side so channel.recv will receive it
        let send_task = tokio::spawn(async move {
            let _ = tun_b.send_all(b"payload").await;
        });

        let (buf, n, _node, _channel) =
            Simulator::generate_channel_reads("node".to_string(), ch.clone())
                .await
                .expect("generate ok");

        assert_eq!(n, 7);
        assert_eq!(&buf[..n], b"payload");

        send_task.await.expect("send task");
    }
}

#[derive(Clone)]
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
pub enum SimNode {
    Obu(Arc<dyn Node>),
    Rsu(Arc<dyn Node>),
    #[allow(dead_code)]
    Server(Arc<Server>),
}

impl SimNode {
    #[cfg(feature = "webview")]
    pub fn as_any(&self) -> &dyn std::any::Any {
        match self {
            SimNode::Obu(o) => o.as_any(),
            SimNode::Rsu(r) => r.as_any(),
            SimNode::Server(_) => {
                // Server nodes don't implement the Node trait, return a dummy
                // This is used for stats collection which doesn't apply to Server nodes
                &()
            }
        }
    }
}

pub struct Simulator {
    #[allow(dead_code)]
    namespaces: Vec<NamespaceWrapper>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
    /// Keep created nodes so external code (e.g. webview) may query node state.
    /// Stores (Device, NodeInterfaces, SimNode) - NodeInterfaces keeps ALL interfaces alive.
    #[cfg_attr(not(feature = "webview"), allow(dead_code))]
    #[allow(clippy::type_complexity)]
    nodes: HashMap<String, (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)>,
    /// Map node names to their namespace index for server startup
    #[allow(dead_code)]
    node_namespace_map: HashMap<String, usize>,
    /// Real-time simulation metrics
    metrics: Arc<crate::metrics::SimulatorMetrics>,
}

type CallbackReturn = Result<(Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)>;

impl Simulator {
    #[allow(clippy::type_complexity)]
    fn parse_topology(
        config_file: &str,
        callback: impl Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    ) -> Result<(
        HashMap<String, HashMap<String, Arc<Channel>>>,
        Vec<NamespaceWrapper>,
        HashMap<String, (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)>,
        HashMap<String, usize>, // node_name -> namespace_index mapping
    )> {
        // Parse topology configuration from file
        let topology_config = crate::topology::TopologyConfig::from_file(config_file)?;

        // Create namespace manager
        let mut ns_manager = NamespaceManager::new();
        let mut node_map: HashMap<
            String,
            (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode),
        > = HashMap::new();
        let mut channels: HashMap<String, HashMap<String, Arc<Channel>>> = HashMap::new();

        // Create namespaces and nodes
        for (node, node_params) in &topology_config.nodes {
            match ns_manager.create_namespace(node, node_params, callback.clone()) {
                Ok((_namespace_idx, device)) => {
                    // Insert node into node_map
                    node_map.insert(node.clone(), device.clone());

                    // Create channels for this node's connections
                    for (tnode, connections) in &topology_config.connections {
                        let Some(parameters) = connections.get(node) else {
                            continue;
                        };

                        // Get VANET interface for channel creation (if it exists)
                        // Servers don't have VANET interfaces, so skip channel creation for them
                        let Some(vanet_tun) = device.1.vanet() else {
                            tracing::debug!(node = %node, "Node has no VANET interface, skipping channel");
                            continue;
                        };

                        channels.entry(tnode.to_string()).or_default().insert(
                            node.to_string(),
                            Channel::new(
                                *parameters,
                                device.0.mac_address(),
                                vanet_tun.clone(),
                                tnode,
                                node,
                            ),
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(node = %node, error = %e, "Failed to create node namespace");
                }
            }
        }

        // Extract namespaces and mapping from manager
        let (namespaces, node_namespace_map) = ns_manager.into_parts();

        Ok((channels, namespaces, node_map, node_namespace_map))
    }

    pub fn new<F>(args: &SimArgs, callback: F) -> Result<Self>
    where
        F: Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    {
        let (channels, namespaces, nodes, node_namespace_map) =
            Self::parse_topology(&args.config_file, callback)?;

        // Initialize metrics
        let metrics = Arc::new(crate::metrics::SimulatorMetrics::new());
        metrics.set_active_nodes(nodes.len() as u64);

        // Count total channels
        let total_channels: usize = channels.values().map(|m| m.len()).sum();
        metrics.set_active_channels(total_channels as u64);

        Ok(Self {
            namespaces,
            channels,
            nodes,
            node_namespace_map,
            metrics,
        })
    }

    pub async fn run(&self) -> Result<()> {
        let mut future_set = self
            .channels
            .values()
            .flat_map(|x| x.iter())
            .unique_by(|(node, _)| *node)
            .map(|(node, channel)| Self::generate_channel_reads(node.to_string(), channel.clone()))
            .collect::<FuturesUnordered<_>>();

        let channel_map_vec: HashMap<&String, Vec<Arc<Channel>>> = self
            .channels
            .iter()
            .map(|(from, map_to)| (from, map_to.values().cloned().collect_vec()))
            .collect();

        loop {
            if let Some(Ok((buf, size, node, channel))) = future_set.next().await {
                if let Some(connections) = channel_map_vec.get(&node) {
                    for channel in connections {
                        let from = channel.from();
                        let to = channel.to();
                        match channel.send(buf, size).await {
                            Ok(_) => {
                                self.metrics.record_packet_sent_for_channel(from, to, size);
                                // Record the latency that will be applied to this packet
                                let params = channel.params();
                                self.metrics.record_packet_delayed(params.latency);
                                self.metrics
                                    .record_latency_for_channel(from, to, params.latency);
                            }
                            Err(crate::channel::ChannelError::Dropped) => {
                                // Count actual packet loss
                                self.metrics.record_packet_dropped_for_channel(from, to);
                            }
                            Err(crate::channel::ChannelError::Filtered) => {
                                // Silently ignore filtered packets - that's expected MAC filtering
                            }
                        }
                    }
                }

                future_set.push(Self::generate_channel_reads(node, channel));
            }
        }
    }

    async fn generate_channel_reads(
        node: String,
        channel: Arc<Channel>,
    ) -> Result<([u8; PACKET_BUFFER_SIZE], usize, String, Arc<Channel>), Error> {
        let mut buf: [u8; PACKET_BUFFER_SIZE] = [0u8; PACKET_BUFFER_SIZE];
        let n = channel.recv(&mut buf).await?;
        Ok((buf, n, node, channel))
    }

    #[cfg(feature = "webview")]
    pub fn get_channels(&self) -> HashMap<String, HashMap<String, Arc<Channel>>> {
        self.channels.clone()
    }

    /// Get real-time simulation metrics.
    pub fn get_metrics(&self) -> Arc<crate::metrics::SimulatorMetrics> {
        self.metrics.clone()
    }

    /// Return a clone of the created nodes with full interface information
    /// (name -> (dev, interfaces, node)).
    #[allow(dead_code)]
    #[allow(clippy::type_complexity)]
    pub fn get_nodes_with_interfaces(
        &self,
    ) -> HashMap<String, (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)> {
        self.nodes.clone()
    }

    /// Return a clone of the created nodes (name -> (dev, tun, node)).
    /// For backward compatibility, this returns only the VANET interface.
    /// Use get_nodes_with_interfaces() for full interface access.
    #[cfg(feature = "webview")]
    #[allow(clippy::type_complexity)]
    pub fn get_nodes(&self) -> HashMap<String, (Arc<Device>, Arc<Tun>, SimNode)> {
        self.nodes
            .iter()
            .filter_map(|(name, (device, interfaces, node))| {
                // Only return nodes with VANET interfaces (OBU/RSU)
                interfaces
                    .vanet()
                    .map(|vanet| (name.clone(), (device.clone(), vanet.clone(), node.clone())))
            })
            .collect()
    }
}
