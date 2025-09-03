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
use node_lib::Node;
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
        from: &String,
        to: &String,
    ) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tracing::info!(from, to, ?parameters, "Created channel");
        let this = Arc::new(Self {
            tx,
            parameters: parameters.into(),
            mac,
            tun,
            queue: VecDeque::with_capacity(1024).into(),
        });
        let thisc = this.clone();
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

    fn should_send(&self, buf: &[u8]) -> Result<()> {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            bail!("not the right mac address")
        }

        let loss = self.parameters.read().unwrap().loss;
        if loss > 0.0 {
            let mut rng = rand::thread_rng();
            if rand::Rng::gen::<f64>(&mut rng) < loss {
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

pub struct Simulator {
    _namespaces: Vec<NamespaceWrapper>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
    /// Keep created nodes so external code (e.g. webview) may query node state.
    #[allow(dead_code)]
    #[allow(clippy::type_complexity)]
    nodes: HashMap<String, (Arc<Device>, Arc<Tun>, Arc<dyn Node>)>,
}

type CallbackReturn = Result<(Arc<Device>, Arc<Tun>, Arc<dyn Node>)>;

impl Simulator {
    #[allow(clippy::type_complexity)]
    fn parse_topology(
        config_file: &str,
        callback: impl Fn(&str, &HashMap<String, Value>) -> CallbackReturn + Clone,
    ) -> Result<(
        HashMap<String, HashMap<String, Arc<Channel>>>,
        Vec<NamespaceWrapper>,
        HashMap<String, (Arc<Device>, Arc<Tun>, Arc<dyn Node>)>,
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
    ) -> Result<([u8; 1500], usize, String, Arc<Channel>), Error> {
        let mut buf: [u8; 1500] = [0u8; 1500];
        let n = channel.recv(&mut buf).await?;
        Ok((buf, n, node, channel))
    }

    #[allow(dead_code)]
    pub fn get_channels(&self) -> HashMap<String, HashMap<String, Arc<Channel>>> {
        self.channels.clone()
    }

    /// Return a clone of the created nodes (name -> (dev, tun, node)).
    #[allow(dead_code, clippy::type_complexity)]
    pub fn get_nodes(&self) -> HashMap<String, (Arc<Device>, Arc<Tun>, Arc<dyn Node>)> {
        self.nodes.clone()
    }
}
