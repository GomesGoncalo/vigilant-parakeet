use crate::node_factory::UnifiedNode;
use crate::sim_args::SimArgs;
use anyhow::Context;
use anyhow::{bail, Error, Result};
use common::channel_parameters::ChannelParameters;
use common::device::Device;
use common::network_interface::NetworkInterface;
use common::tun::Tun;
use config::Config;
use config::Value;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use mac_address::MacAddress;
use netns_rs::NetNs;
use rand::Rng;
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
// uninit_array is not used here

/// Return a compact hex string for a byte slice (e.g. "01 02 aa ...").
pub fn bytes_to_hex(slice: &[u8]) -> String {
    slice
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct NamespaceWrapper(Option<NetNs>);

impl NamespaceWrapper {
    fn new(ns: NetNs) -> Self {
        Self(Some(ns))
    }
}

impl Drop for NamespaceWrapper {
    fn drop(&mut self) {
        let Some(ns) = self.0.take() else {
            panic!("No value inside?");
        };
        let _ = ns.remove();
    }
}

struct Packet {
    packet: [u8; 1500],
    size: usize,
    instant: Instant,
}

pub struct Channel {
    tx: UnboundedSender<()>,
    parameters: RwLock<ChannelParameters>,
    mac: MacAddress,
    tun: Arc<Tun>,
    queue: Mutex<VecDeque<Packet>>,
    // Add device forwarding
    device_tx: UnboundedSender<()>,
    device: Arc<Device>,
    device_queue: Mutex<VecDeque<Packet>>,
}

#[allow(dead_code)]
impl Channel {
    pub fn params(&self) -> ChannelParameters {
        *self.parameters.read().unwrap()
    }

    pub fn set_params(&self, params: HashMap<String, String>) -> Result<()> {
        let result = ChannelParameters {
            latency: Duration::from_millis(
                (params.get("latency").context("could not get latency")?).parse::<u64>()?,
            ),
            loss: f64::from_str(params.get("loss").context("could not get loss")?)?,
        };

        let mut inner_params = self.parameters.write().unwrap();
        *inner_params = result;
        let _ = self.tx.send(());
        Ok(())
    }

    pub fn new(
        parameters: ChannelParameters,
        mac: MacAddress,
        tun: Arc<Tun>,
        device: Arc<Device>,
        from: &String,
        to: &String,
    ) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (device_tx, mut device_rx) = tokio::sync::mpsc::unbounded_channel();
        tracing::info!(from, to, ?parameters, "Created channel with device forwarding support");
        let this = Arc::new(Self {
            tx,
            parameters: parameters.into(),
            mac,
            tun: tun.clone(),
            queue: VecDeque::with_capacity(1024).into(),
            device_tx,
            device: device.clone(),
            device_queue: VecDeque::with_capacity(1024).into(),
        });
        let thisc = this.clone();
        
        // TUN forwarding task (existing logic)
        tokio::spawn(async move {
            loop {
                let Some(packet) = thisc.queue.lock().unwrap().pop_front() else {
                    let _ = rx.recv().await;
                    continue;
                };
                loop {
                    let latency = thisc.parameters.read().unwrap().latency;
                    let duration = (packet.instant + latency).duration_since(Instant::now());
                    if duration.is_zero() {
                        let _ = thisc.tun.send_all(&packet.packet[..packet.size]).await;
                        break;
                    } else {
                        tokio::select! {
                            _ = tokio_timerfd::sleep(duration) => {
                                let _ = thisc.tun.send_all(&packet.packet[..packet.size]).await;
                                break;
                            },
                            _ = rx.recv() => {},
                        }
                    }
                }
            }
        });

        let thisc2 = this.clone();
        // Device forwarding task (new logic)
        tokio::spawn(async move {
            loop {
                let Some(packet) = thisc2.device_queue.lock().unwrap().pop_front() else {
                    let _ = device_rx.recv().await;
                    continue;
                };
                loop {
                    let latency = thisc2.parameters.read().unwrap().latency;
                    let duration = (packet.instant + latency).duration_since(Instant::now());
                    if duration.is_zero() {
                        let slices = vec![std::io::IoSlice::new(&packet.packet[..packet.size])];
                        match thisc2.device.send_vectored(&slices).await {
                            Ok(bytes_sent) => {
                                tracing::trace!(bytes_sent = bytes_sent, "Device forwarding sent packet successfully");
                            }
                            Err(e) => {
                                tracing::debug!("Device forwarding failed to send packet: {}", e);
                            }
                        }
                        break;
                    } else {
                        tokio::select! {
                            _ = tokio_timerfd::sleep(duration) => {
                                let slices = vec![std::io::IoSlice::new(&packet.packet[..packet.size])];
                                match thisc2.device.send_vectored(&slices).await {
                                    Ok(bytes_sent) => {
                                        tracing::trace!(bytes_sent = bytes_sent, "Device forwarding sent delayed packet successfully");
                                    }
                                    Err(e) => {
                                        tracing::debug!("Device forwarding failed to send delayed packet: {}", e);
                                    }
                                }
                                break;
                            },
                            _ = device_rx.recv() => {},
                        }
                    }
                }
            }
        });
        
        this
    }

    pub async fn send(&self, packet: [u8; 1500], size: usize) -> Result<()> {
        self.should_send(&packet[..size])?;
        let mut queue = self.queue.lock().unwrap();
        if queue.is_empty() {
            let _ = self.tx.send(());
        }
        queue.push_back(Packet {
            packet,
            size,
            instant: Instant::now(),
        });
        Ok(())
    }

    pub async fn send_device(&self, packet: [u8; 1500], size: usize) -> Result<()> {
        self.should_send(&packet[..size])?;
        let mut queue = self.device_queue.lock().unwrap();
        if queue.is_empty() {
            let _ = self.device_tx.send(());
        }
        queue.push_back(Packet {
            packet,
            size,
            instant: Instant::now(),
        });
        Ok(())
    }

    fn should_send(&self, buf: &[u8]) -> Result<()> {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            bail!("not the right mac address")
        }

        let loss = self.parameters.read().unwrap().loss;
        if loss > 0.0 {
            let mut rng = rand::rng();
            if rng.random::<f64>() < loss {
                bail!("packet lost")
            }
        }

        Ok(())
    }

    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let n = self.tun.recv(buf).await?;
        Ok(n)
    }

    pub async fn recv_device(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        // Add timeout to prevent blocking indefinitely on device reads
        match tokio::time::timeout(std::time::Duration::from_millis(100), self.device.recv(buf)).await {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Device recv timeout")),
        }
    }
}

pub struct Simulator {
    _namespaces: Vec<NamespaceWrapper>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
    /// Keep created nodes so external code (e.g. webview) may query node state.
    #[allow(dead_code)]
    #[allow(clippy::type_complexity)]
    nodes: HashMap<String, (Arc<Device>, Arc<Tun>, UnifiedNode)>,
}

type CallbackReturn = Result<(Arc<Device>, Arc<Tun>, UnifiedNode)>;

impl Simulator {
    #[allow(clippy::type_complexity)]
    fn parse_topology(
        config_file: &str,
        callback: impl Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    ) -> Result<(
        HashMap<String, HashMap<String, Arc<Channel>>>,
        Vec<NamespaceWrapper>,
        HashMap<String, (Arc<Device>, Arc<Tun>, UnifiedNode)>,
    )> {
        let settings = Config::builder()
            .add_source(config::File::with_name(config_file))
            .build()?;

        let nodes = settings
            .get_table("nodes")?
            .iter()
            .filter_map(|(node, val)| {
                let Ok(param) = val.clone().into_table() else {
                    return None;
                };
                Some((node.clone(), param))
            })
            .collect::<HashMap<_, _>>();

        tracing::debug!("Parsed nodes: {:?}", nodes.keys().collect::<Vec<_>>());

        let topology = settings.get_table("topology")?;
        let topology: HashMap<String, HashMap<String, ChannelParameters>> = topology
            .iter()
            .map(|(key, val)| {
                let val = val.clone().into_table().unwrap_or_default();
                (
                    key.clone(),
                    val.iter()
                        .map(|(onode, param)| {
                            let param = param.clone().into_table().unwrap_or_default();
                            let param = ChannelParameters::from(param);
                            (onode.clone(), param)
                        })
                        .collect(),
                )
            })
            .collect();

        tracing::debug!("Parsed topology: {:#?}", topology);

        Ok(nodes.iter().fold(
            (HashMap::default(), Vec::default(), HashMap::default()),
            |(channels, mut namespaces, mut node_map), (node, node_params)| {
                tracing::debug!("Processing node: {}", node);
                let Ok(device) =
                    Self::create_namespaces(&mut namespaces, node, node_params, callback.clone())
                else {
                    tracing::error!("Failed to create namespace for node: {}", node);
                    return (channels, namespaces, node_map);
                };

                tracing::debug!("Successfully created device for node: {}", node);

                // Insert node into node_map for later querying.
                node_map.insert(node.clone(), device.clone());

                let new_channels = topology
                    .iter()
                    .fold(channels, |mut channels, (tnode, connections)| {
                        tracing::debug!("Checking topology from {} to {}", tnode, node);
                        let Some(parameters) = connections.get(node) else {
                            tracing::debug!("No connection from {} to {}", tnode, node);
                            return channels;
                        };

                        tracing::debug!("Creating channel from {} to {} with params: {:?}", tnode, node, parameters);

                        channels.entry(tnode.to_string()).or_default().insert(
                            node.to_string(),
                            Channel::new(
                                *parameters,
                                device.0.mac_address(),
                                device.1.clone(),
                                device.0.clone(),
                                tnode,
                                node,
                            ),
                        );
                        channels
                    });
                
                tracing::debug!("Channels after processing node {}: {:?}", node, new_channels.keys().collect::<Vec<_>>());
                
                (
                    new_channels,
                    namespaces,
                    node_map,
                )
            },
        ))
    }

    fn create_namespaces(
        ns_list: &mut Vec<NamespaceWrapper>,
        node: &str,
        node_type: &HashMap<String, Value>,
        callback: impl Fn(&str, &HashMap<String, Value>) -> CallbackReturn,
    ) -> CallbackReturn {
        let node_name = format!("sim_ns_{node}");
        tracing::debug!("Creating namespace: {}", node_name);
        
        let ns = match NetNs::new(node_name.clone()) {
            Ok(ns) => NamespaceWrapper::new(ns),
            Err(e) => {
                tracing::error!("Failed to create namespace {}: {}", node_name, e);
                bail!("no namespace creation: {}", e);
            }
        };
        
        let Some(nsi) = ns.0.as_ref() else {
            tracing::error!("Namespace wrapper is empty for {}", node_name);
            bail!("no namespace");
        };
        
        tracing::debug!("Running callback in namespace {}", node_name);
        // Avoid creating an &&str by passing `node` directly
        let result = nsi.run(|_| callback(node, node_type));
        let Ok(Ok(device)) = result else {
            tracing::error!("Callback failed in namespace {}", node_name);
            bail!("error creating namespace callback");
        };
        
        tracing::debug!("Successfully created node {} in namespace {}", node, node_name);
        ns_list.push(ns);
        Ok(device)
    }

    pub fn new<F>(args: &SimArgs, callback: F) -> Result<Self>
    where
        F: Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    {
        let (channels, namespaces, nodes) = Self::parse_topology(&args.config_file, callback)?;
        Ok(Self {
            _namespaces: namespaces,
            channels,
            nodes,
        })
    }

    pub async fn run(&self) -> Result<()> {
        // Debug: log the channel structure
        tracing::debug!("Channel structure: {:#?}", self.channels.keys().collect::<Vec<_>>());
        for (from, to_map) in &self.channels {
            tracing::debug!("From {}: connections to {:?}", from, to_map.keys().collect::<Vec<_>>());
        }

        // TUN forwarding tasks (existing logic)
        let mut tun_future_set = self
            .channels
            .values()
            .flat_map(|x| x.iter())
            .unique_by(|(node, _)| *node)
            .map(|(node, channel)| Self::generate_tun_reads(node.to_string(), channel.clone()))
            .collect::<FuturesUnordered<_>>();

        // Device forwarding tasks (new logic)
        let mut device_future_set = self
            .channels
            .values()
            .flat_map(|x| x.iter())
            .unique_by(|(node, _)| *node)
            .map(|(node, channel)| Self::generate_device_reads(node.to_string(), channel.clone()))
            .collect::<FuturesUnordered<_>>();

        tracing::info!("Starting simulator with {} TUN channels and {} device channels", tun_future_set.len(), device_future_set.len());

        let channel_map_vec: HashMap<&String, Vec<Arc<Channel>>> = self
            .channels
            .iter()
            .map(|(from, map_to)| (from, map_to.values().cloned().collect_vec()))
            .collect();

        loop {
            tokio::select! {
                // Handle TUN traffic forwarding
                tun_result = tun_future_set.next(), if !tun_future_set.is_empty() => {
                    if let Some(Ok((buf, size, node, channel))) = tun_result {
                        if let Some(connections) = channel_map_vec.get(&node) {
                            for target_channel in connections {
                                let _ = target_channel.send(buf, size).await;
                            }
                        }
                        tun_future_set.push(Self::generate_tun_reads(node, channel));
                    }
                }
                // Handle device traffic forwarding
                device_result = device_future_set.next(), if !device_future_set.is_empty() => {
                    if let Some(Ok((buf, size, node, channel))) = device_result {
                        tracing::debug!(node = node, size = size, "Forwarding device traffic from node");
                        if let Some(connections) = channel_map_vec.get(&node) {
                            for target_channel in connections {
                                match target_channel.send_device(buf, size).await {
                                    Ok(_) => {
                                        tracing::debug!(to_node = %target_channel.mac, "Successfully forwarded device traffic");
                                    }
                                    Err(e) => {
                                        tracing::debug!("Failed to forward device traffic: {}", e);
                                    }
                                }
                            }
                        }
                        device_future_set.push(Self::generate_device_reads(node, channel));
                    }
                }
                // Fallback in case both sets are empty (shouldn't happen in normal operation)
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if tun_future_set.is_empty() && device_future_set.is_empty() {
                        tracing::warn!("Both TUN and device future sets are empty, simulator may have stopped");
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn generate_tun_reads(
        node: String,
        channel: Arc<Channel>,
    ) -> Result<([u8; 1500], usize, String, Arc<Channel>), Error> {
        let mut buf: [u8; 1500] = [0u8; 1500];
        let n = channel.recv(&mut buf).await?;
        Ok((buf, n, node, channel))
    }

    async fn generate_device_reads(
        node: String,
        channel: Arc<Channel>,
    ) -> Result<([u8; 1500], usize, String, Arc<Channel>), Error> {
        let mut buf: [u8; 1500] = [0u8; 1500];
        loop {
            match channel.recv_device(&mut buf).await {
                Ok(n) => {
                    tracing::debug!(node = node, size = n, raw = %bytes_to_hex(&buf[..n.min(32)]), "Read device traffic from node");
                    return Ok((buf, n, node, channel));
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Timeout is expected for device reads, just retry
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    #[allow(dead_code)]
    pub fn get_channels(&self) -> HashMap<String, HashMap<String, Arc<Channel>>> {
        self.channels.clone()
    }

    /// Return a clone of the created nodes (name -> (dev, tun, node)).
    #[allow(dead_code, clippy::type_complexity)]
    pub fn get_nodes(&self) -> HashMap<String, (Arc<Device>, Arc<Tun>, UnifiedNode)> {
        self.nodes.clone()
    }
}
