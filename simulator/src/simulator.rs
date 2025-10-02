use crate::sim_args::SimArgs;
#[cfg(any(test, feature = "webview"))]
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
use node_lib::{Node, PACKET_BUFFER_SIZE};
use rand::Rng;
#[cfg(any(test, feature = "webview"))]
use std::str::FromStr;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
// uninit_array is not used here

pub struct NamespaceWrapper(Option<NetNs>);

impl NamespaceWrapper {
    fn new(ns: NetNs) -> Self {
        Self(Some(ns))
    }
}

#[cfg(test)]
mod simulator_tests {
    use super::*;
    use common::channel_parameters::ChannelParameters;
    use mac_address::MacAddress;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn channel_set_params_updates_and_allows_send_simfile() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "0".to_string());

        assert!(ch.set_params(map).is_ok());

        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&mac.bytes());
        packet[6] = 0x42;

        assert!(ch.send(packet, 7).await.is_ok());
    }

    #[tokio::test]
    async fn channel_send_wrong_mac_fails() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // packet with a different destination MAC
        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&[9u8, 9, 9, 9, 9, 9]);
        let res = ch.send(packet, 7).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn channel_send_forced_loss() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        // set params with loss = 1.0 to force packet drop
        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "1.0".to_string());

        let params = ChannelParameters::from(HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);
        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );
        assert!(ch.set_params(map).is_ok());

        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&mac.bytes());
        let res = ch.send(packet, 7).await;
        // With loss=1.0, should_send will bail and send returns Err
        assert!(res.is_err());
    }

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

impl Drop for NamespaceWrapper {
    fn drop(&mut self) {
        let Some(ns) = self.0.take() else {
            panic!("No value inside?");
        };
        let _ = ns.remove();
    }
}

struct Packet {
    packet: [u8; PACKET_BUFFER_SIZE],
    size: usize,
    instant: Instant,
}

pub struct Channel {
    tx: mpsc::UnboundedSender<Packet>,
    #[cfg_attr(not(feature = "webview"), allow(dead_code))]
    param_notify_tx: mpsc::UnboundedSender<()>,
    parameters: RwLock<ChannelParameters>,
    mac: MacAddress,
    tun: Arc<Tun>,
}

impl Channel {
    #[cfg(any(test, feature = "webview"))]
    #[allow(dead_code)]
    pub fn params(&self) -> ChannelParameters {
        *self
            .parameters
            .read()
            .expect("channel parameters lock poisoned")
    }

    #[cfg(any(test, feature = "webview"))]
    pub fn set_params(&self, params: HashMap<String, String>) -> Result<()> {
        let result = ChannelParameters {
            latency: Duration::from_millis(
                (params.get("latency").context("could not get latency")?).parse::<u64>()?,
            ),
            loss: f64::from_str(params.get("loss").context("could not get loss")?)?,
            jitter: Duration::from_millis(
                params.get("jitter").unwrap_or(&"0".to_string()).parse::<u64>()?,
            ),
        };

        let mut inner_params = self
            .parameters
            .write()
            .expect("channel parameters lock poisoned");
        *inner_params = result;
        let _ = self.param_notify_tx.send(());
        Ok(())
    }

    pub fn new(
        parameters: ChannelParameters,
        mac: MacAddress,
        tun: Arc<Tun>,
        from: &String,
        to: &String,
    ) -> Arc<Self> {
        // Use unbounded channels for zero-copy fast path
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (param_notify_tx, mut param_notify_rx) = mpsc::unbounded_channel();

        tracing::info!(from, to, ?parameters, "Created channel");
        let this = Arc::new(Self {
            tx,
            param_notify_tx,
            parameters: parameters.into(),
            mac,
            tun,
        });
        let thisc = this.clone();

        // Spawn task to process packets from channel with latency simulation
        tokio::spawn(async move {
            loop {
                let Some(packet) = rx.recv().await else {
                    // Channel closed, exit task
                    break;
                };

                loop {
                    // Calculate latency with jitter in a separate scope to drop the lock
                    let latency = {
                        let params = thisc
                            .parameters
                            .read()
                            .expect("channel parameters lock poisoned");
                        
                        // Apply base latency + random jitter
                        let mut latency = params.latency;
                        if !params.jitter.is_zero() {
                            let mut rng = rand::rng();
                            // Generate random jitter in range [-jitter, +jitter]
                            let jitter_ms = params.jitter.as_millis() as i64;
                            let random_jitter = rng.random_range(-jitter_ms..=jitter_ms);
                            if random_jitter >= 0 {
                                latency += Duration::from_millis(random_jitter as u64);
                            } else {
                                // Subtract jitter, but don't go negative
                                let abs_jitter = (-random_jitter) as u64;
                                if latency > Duration::from_millis(abs_jitter) {
                                    latency -= Duration::from_millis(abs_jitter);
                                } else {
                                    latency = Duration::ZERO;
                                }
                            }
                        }
                        latency
                    }; // Lock is released here
                    
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
                            _ = param_notify_rx.recv() => {
                                // Parameters changed, recalculate duration
                            },
                        }
                    }
                }
            }
        });
        this
    }

    pub async fn send(&self, packet: [u8; PACKET_BUFFER_SIZE], size: usize) -> Result<()> {
        self.should_send(&packet[..size])?;

        // Send packet through unbounded channel - no blocking on fast path
        self.tx
            .send(Packet {
                packet,
                size,
                instant: Instant::now(),
            })
            .map_err(|_| anyhow::anyhow!("channel send failed"))?;

        Ok(())
    }

    fn should_send(&self, buf: &[u8]) -> Result<()> {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            bail!("not the right mac address")
        }

        let loss = self
            .parameters
            .read()
            .expect("channel parameters lock poisoned")
            .loss;
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
}

#[derive(Clone)]
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
pub enum SimNode {
    Obu(Arc<dyn Node>),
    Rsu(Arc<dyn Node>),
}

impl SimNode {
    #[cfg(feature = "webview")]
    pub fn as_any(&self) -> &dyn std::any::Any {
        match self {
            SimNode::Obu(o) => o.as_any(),
            SimNode::Rsu(r) => r.as_any(),
        }
    }
}

pub struct Simulator {
    _namespaces: Vec<NamespaceWrapper>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
    /// Keep created nodes so external code (e.g. webview) may query node state.
    #[cfg_attr(not(feature = "webview"), allow(dead_code))]
    #[allow(clippy::type_complexity)]
    nodes: HashMap<String, (Arc<Device>, Arc<Tun>, SimNode)>,
}

type CallbackReturn = Result<(Arc<Device>, Arc<Tun>, SimNode)>;

impl Simulator {
    #[allow(clippy::type_complexity)]
    fn parse_topology(
        config_file: &str,
        callback: impl Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    ) -> Result<(
        HashMap<String, HashMap<String, Arc<Channel>>>,
        Vec<NamespaceWrapper>,
        HashMap<String, (Arc<Device>, Arc<Tun>, SimNode)>,
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

        Ok(nodes.iter().fold(
            (HashMap::default(), Vec::default(), HashMap::default()),
            |(channels, mut namespaces, mut node_map), (node, node_params)| {
                let Ok(device) =
                    Self::create_namespaces(&mut namespaces, node, node_params, callback.clone())
                else {
                    return (channels, namespaces, node_map);
                };

                // Insert node into node_map for later querying.
                node_map.insert(node.clone(), device.clone());

                (
                    topology
                        .iter()
                        .fold(channels, |mut channels, (tnode, connections)| {
                            let Some(parameters) = connections.get(node) else {
                                return channels;
                            };

                            channels.entry(tnode.to_string()).or_default().insert(
                                node.to_string(),
                                Channel::new(
                                    *parameters,
                                    device.0.mac_address(),
                                    device.1.clone(),
                                    tnode,
                                    node,
                                ),
                            );
                            channels
                        }),
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
        let ns = NamespaceWrapper::new(NetNs::new(node_name.clone())?);
        let Some(nsi) = ns.0.as_ref() else {
            bail!("no namespace");
        };
        // Avoid creating an &&str by passing `node` directly
        let Ok(Ok(device)) = nsi.run(|_| callback(node, node_type)) else {
            bail!("error creating namespace");
        };
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
                        let _ = channel.send(buf, size).await;
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

    /// Return a clone of the created nodes (name -> (dev, tun, node)).
    #[cfg(feature = "webview")]
    #[allow(clippy::type_complexity)]
    pub fn get_nodes(&self) -> HashMap<String, (Arc<Device>, Arc<Tun>, SimNode)> {
        self.nodes.clone()
    }
}
