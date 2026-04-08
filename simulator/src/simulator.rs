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
#[cfg(feature = "mobility")]
use {
    crate::fading::NakagamiConfig,
    crate::mobility::{position::NodePosition, MobilityConfig, MobilityManager, NodeGeoConfig},
    common::channel_parameters::ChannelParameters,
    std::time::Duration,
    tokio::sync::RwLock,
};

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
    /// Shared geographic positions updated by the mobility manager (if enabled).
    #[cfg(feature = "mobility")]
    positions: Arc<RwLock<HashMap<String, NodePosition>>>,
    /// Override queue: HTTP layer posts (lat, lon) here; tick loop replans.
    #[cfg(feature = "mobility")]
    override_queue: Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>>,
    /// Mobility manager (held here so run() can spawn its tick loop).
    #[cfg(feature = "mobility")]
    mobility_manager: tokio::sync::Mutex<Option<MobilityManager>>,
    /// Nakagami-m fading config — present when fading is enabled.
    #[cfg(feature = "mobility")]
    nakagami_config: Option<NakagamiConfig>,
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

        // Second pass: create bidirectional cloud channels for RSU ↔ Server connections.
        //
        // First honour any explicit topology entries (preserves custom latency/loss),
        // then auto-create cloud channels for any RSU↔Server pair not yet connected.
        // This means the topology: section is fully optional — cloud connectivity is
        // always established automatically.
        let zero_params = common::channel_parameters::ChannelParameters {
            latency: std::time::Duration::ZERO,
            loss: 0.0,
            jitter: std::time::Duration::ZERO,
        };
        let no_filter_mac = mac_address::MacAddress::new([0u8; 6]);

        // Helper closure — create bidirectional cloud channel if not already present.
        let add_cloud_channel =
            |channels: &mut HashMap<String, HashMap<String, Arc<Channel>>>,
             from_node: &str,
             to_node: &str,
             params: common::channel_parameters::ChannelParameters,
             from_cloud: &Arc<common::tun::Tun>,
             to_cloud: &Arc<common::tun::Tun>| {
                let from_key = format!("{}:cloud", from_node);
                let to_key = format!("{}:cloud", to_node);
                channels.entry(from_key.clone()).or_default().entry(to_key.clone()).or_insert_with(|| {
                    Channel::new(params, no_filter_mac, to_cloud.clone(), &from_key, &to_key)
                });
                channels.entry(to_key.clone()).or_default().entry(from_key.clone()).or_insert_with(|| {
                    Channel::new(params, no_filter_mac, from_cloud.clone(), &to_key, &from_key)
                });
                tracing::info!(from = %from_node, to = %to_node, "Created bidirectional cloud channel");
            };

        // Explicit topology entries (custom params).
        for (from_node, connections) in &topology_config.connections {
            for (to_node, params) in connections {
                let (Some(from_entry), Some(to_entry)) =
                    (node_map.get(from_node), node_map.get(to_node))
                else {
                    continue;
                };

                let from_is_server = matches!(from_entry.2, SimNode::Server(_));
                let to_is_server = matches!(to_entry.2, SimNode::Server(_));
                if !from_is_server && !to_is_server {
                    continue;
                }

                let (Some(from_cloud), Some(to_cloud)) =
                    (from_entry.1.cloud.as_ref(), to_entry.1.cloud.as_ref())
                else {
                    tracing::warn!(
                        from = %from_node,
                        to = %to_node,
                        "Server topology connection found but cloud interface missing on one side"
                    );
                    continue;
                };

                add_cloud_channel(
                    &mut channels,
                    from_node,
                    to_node,
                    *params,
                    from_cloud,
                    to_cloud,
                );
            }
        }

        // Auto-connect any RSU↔Server pair not yet wired (no topology entry needed).
        let server_names: Vec<String> = node_map
            .iter()
            .filter(|(_, (_, _, sn))| matches!(sn, SimNode::Server(_)))
            .map(|(n, _)| n.clone())
            .collect();
        let rsu_names: Vec<String> = node_map
            .iter()
            .filter(|(_, (_, _, sn))| matches!(sn, SimNode::Rsu(_)))
            .map(|(n, _)| n.clone())
            .collect();

        for rsu in &rsu_names {
            for srv in &server_names {
                let from_key = format!("{}:cloud", rsu);
                let to_key = format!("{}:cloud", srv);
                if channels.get(&from_key).and_then(|m| m.get(&to_key)).is_some() {
                    continue; // already added via explicit topology
                }
                let (Some(rsu_entry), Some(srv_entry)) =
                    (node_map.get(rsu), node_map.get(srv))
                else {
                    continue;
                };
                let (Some(rsu_cloud), Some(srv_cloud)) =
                    (rsu_entry.1.cloud.as_ref(), srv_entry.1.cloud.as_ref())
                else {
                    continue;
                };
                add_cloud_channel(
                    &mut channels,
                    rsu,
                    srv,
                    zero_params,
                    rsu_cloud,
                    srv_cloud,
                );
            }
        }

        // Extract namespaces and mapping from manager
        let (namespaces, node_namespace_map) = ns_manager.into_parts();

        Ok((channels, namespaces, node_map, node_namespace_map))
    }

    pub async fn new<F>(args: &SimArgs, callback: F) -> Result<Self>
    where
        F: Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    {
        #[cfg_attr(not(feature = "mobility"), allow(unused_mut))]
        let (mut channels, namespaces, nodes, node_namespace_map) =
            Self::parse_topology(&args.config_file, callback)?;

        // Parse optional Nakagami-m fading config and build full-mesh channels.
        #[cfg(feature = "mobility")]
        let nakagami_config = {
            let cfg = Self::parse_nakagami_config(&args.config_file);
            if let Some(ref c) = cfg {
                if c.enabled {
                    Self::build_full_mesh_channels(&mut channels, &nodes);
                }
            }
            cfg
        };

        // Initialize metrics
        let metrics = Arc::new(crate::metrics::SimulatorMetrics::new());
        metrics.set_active_nodes(nodes.len() as u64);

        // Count total channels
        let total_channels: usize = channels.values().map(|m| m.len()).sum();
        metrics.set_active_channels(total_channels as u64);

        #[cfg(feature = "mobility")]
        let (positions, override_queue, mobility_option) =
            Self::maybe_init_mobility(&args.config_file, &nodes).await;

        Ok(Self {
            namespaces,
            channels,
            nodes,
            node_namespace_map,
            metrics,
            #[cfg(feature = "mobility")]
            nakagami_config,
            #[cfg(feature = "mobility")]
            positions,
            #[cfg(feature = "mobility")]
            override_queue,
            #[cfg(feature = "mobility")]
            mobility_manager: tokio::sync::Mutex::new(mobility_option),
        })
    }

    /// Parse optional `nakagami:` section from the config file.
    #[cfg(feature = "mobility")]
    fn parse_nakagami_config(config_file: &str) -> Option<NakagamiConfig> {
        let cfg = config::Config::builder()
            .add_source(config::File::with_name(config_file))
            .build()
            .ok()?;
        match cfg.get::<NakagamiConfig>("nakagami") {
            Ok(c) if c.enabled => Some(c),
            Ok(_) => None,
            Err(_) => None,
        }
    }

    /// Create directed VANET channels for every ordered non-server pair `(A → B)` that
    /// doesn't already have a channel.  Initial loss = 0 (will be updated by fading task).
    #[cfg(feature = "mobility")]
    fn build_full_mesh_channels(
        channels: &mut HashMap<String, HashMap<String, Arc<Channel>>>,
        nodes: &HashMap<String, (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)>,
    ) {
        let default_params = ChannelParameters {
            latency: Duration::ZERO,
            loss: 0.0,
            jitter: Duration::ZERO,
        };

        let node_list: Vec<_> = nodes
            .iter()
            .filter(|(_, (_, _, sim_node))| !matches!(sim_node, SimNode::Server(_)))
            .map(|(name, (device, interfaces, _))| {
                (name.clone(), device.clone(), interfaces.vanet().cloned())
            })
            .collect();

        for (from, _from_dev, _) in &node_list {
            for (to, to_dev, to_vanet) in &node_list {
                if from == to {
                    continue;
                }
                if channels.get(from).and_then(|m| m.get(to)).is_some() {
                    continue;
                }
                let Some(to_tun) = to_vanet.as_ref() else {
                    continue;
                };
                // MAC filter = destination node's MAC: only frames addressed to `to`
                // (or broadcast) will pass through, matching the convention used in
                // parse_topology where channels[from][to].mac = to's MAC.
                let ch = Channel::new(
                    default_params,
                    to_dev.mac_address(),
                    to_tun.clone(),
                    from,
                    to,
                );
                channels
                    .entry(from.clone())
                    .or_default()
                    .insert(to.clone(), ch);
            }
        }
    }

    /// Parse optional mobility config and per-node geo configs, then init the
    /// MobilityManager if `mobility.enabled` is set.
    #[cfg(feature = "mobility")]
    async fn maybe_init_mobility(
        config_file: &str,
        nodes: &HashMap<String, (Arc<Device>, crate::node_interfaces::NodeInterfaces, SimNode)>,
    ) -> (
        Arc<RwLock<HashMap<String, NodePosition>>>,
        Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>>,
        Option<MobilityManager>,
    ) {
        use config::Config;

        let config_result = Config::builder()
            .add_source(config::File::with_name(config_file))
            .build();

        let mob_config: MobilityConfig = match config_result {
            Ok(cfg) => cfg.get::<MobilityConfig>("mobility").unwrap_or_default(),
            Err(_) => MobilityConfig::default(),
        };

        if !mob_config.enabled {
            return (
                Arc::new(RwLock::new(HashMap::new())),
                Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                None,
            );
        }

        // Build per-node geo configs from individual node YAML files.
        // We read only lat/lon; missing values are treated as None.
        let mut node_configs: HashMap<String, (String, NodeGeoConfig)> = HashMap::new();

        // Extract node type from SimNode and try to read lat/lon from their config files.
        // We re-read the simulator config to get config_path per node.
        let top_config = Config::builder()
            .add_source(config::File::with_name(config_file))
            .build()
            .ok();

        for (name, (_device, _ifaces, sim_node)) in nodes {
            let node_type = match sim_node {
                SimNode::Obu(_) => "Obu",
                SimNode::Rsu(_) => "Rsu",
                SimNode::Server(_) => "Server",
            };

            let mut geo = NodeGeoConfig::default();

            if let Some(ref cfg) = top_config {
                // Try to get config_path from nodes.<name>.config_path
                if let Ok(node_cfg_path) = cfg.get_string(&format!("nodes.{name}.config_path")) {
                    if let Ok(node_cfg) = Config::builder()
                        .add_source(config::File::with_name(&node_cfg_path))
                        .build()
                    {
                        geo.lat = node_cfg.get_float("lat").ok();
                        geo.lon = node_cfg.get_float("lon").ok();
                    }
                }
            }

            node_configs.insert(name.clone(), (node_type.to_string(), geo));
        }

        match MobilityManager::new(mob_config, node_configs).await {
            Ok(mgr) => {
                let pos = mgr.get_positions();
                let oq = mgr.get_override_queue();
                (pos, oq, Some(mgr))
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to initialise MobilityManager — mobility disabled");
                (
                    Arc::new(RwLock::new(HashMap::new())),
                    Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                    None,
                )
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        // Spawn mobility tick loop if the manager was initialised.
        #[cfg(feature = "mobility")]
        {
            if let Some(mgr) = self.mobility_manager.lock().await.take() {
                tracing::info!("Starting mobility tick loop");
                tokio::spawn(mgr.run_loop());
            }
        }

        // Spawn Nakagami-m fading task if enabled (requires mobility positions).
        #[cfg(feature = "mobility")]
        if let Some(ref nak_cfg) = self.nakagami_config {
            tracing::info!("Starting Nakagami-m fading task");
            let positions = self.positions.clone();
            let cfg = nak_cfg.clone();
            // Collect only VANET channels (exclude ":cloud" keys).
            let vanet_channels: Vec<(String, String, Arc<Channel>)> = self
                .channels
                .iter()
                .filter(|(from, _)| !from.contains(":cloud"))
                .flat_map(|(from, to_map)| {
                    to_map
                        .iter()
                        .filter(|(to, _)| !to.contains(":cloud"))
                        .map(|(to, ch)| (from.clone(), to.clone(), ch.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();

            let interval = Duration::from_millis(cfg.update_ms);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    let pos = positions.read().await;
                    for (from, to, channel) in &vanet_channels {
                        if let (Some(fp), Some(tp)) = (pos.get(from), pos.get(to)) {
                            let d = crate::fading::haversine_m(fp.lat, fp.lon, tp.lat, tp.lon);
                            let loss = crate::fading::nakagami_loss(d, &cfg);
                            // Scale latency by distance so routing prefers closer RSUs.
                            let latency_ms =
                                ((d / 100.0) * cfg.latency_ms_per_100m).round() as u64;
                            channel.set_fading_params(loss, latency_ms);
                        }
                    }
                }
            });
        }

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

    /// Get the shared positions map updated by the mobility manager.
    #[cfg(feature = "mobility")]
    pub fn get_positions(&self) -> Arc<RwLock<HashMap<String, NodePosition>>> {
        self.positions.clone()
    }

    /// Get a reference to the mobility manager mutex for position overrides.
    #[cfg(feature = "mobility")]
    #[allow(dead_code)]
    fn mobility_manager(&self) -> &tokio::sync::Mutex<Option<MobilityManager>> {
        &self.mobility_manager
    }

    /// Get the position override queue — write (name, lat, lon) here to replan a vehicle.
    #[cfg(feature = "mobility")]
    pub fn get_override_queue(&self) -> Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>> {
        // The queue lives inside the MobilityManager; we hold a pre-extracted Arc.
        self.override_queue.clone()
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
