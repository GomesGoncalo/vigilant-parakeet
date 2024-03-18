use crate::sim_args::SimArgs;
use anyhow::Context;
use anyhow::{bail, Error, Result};
use config::Config;
use config::Value;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use mac_address::MacAddress;
use netns_rs::NetNs;
use node_lib::dev::Device;
use serde::Serialize;
use std::str::FromStr;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::mpsc::Sender;
use tokio_tun::Tun;
use uninit::uninit_array;

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

#[derive(Debug, Copy, Clone, Serialize)]
pub struct ChannelParameters {
    latency: Duration,
    loss: f64,
}

impl From<HashMap<String, Value>> for ChannelParameters {
    fn from(param: HashMap<String, Value>) -> Self {
        let latency = match param.get("latency") {
            Some(val) => val.clone().into_uint().unwrap_or(0),
            None => 0,
        };
        let loss = match param.get("loss") {
            Some(val) => val.clone().into_float().unwrap_or(0.0),
            None => 0.0,
        };

        Self {
            latency: Duration::from_millis(latency),
            loss,
        }
    }
}

pub struct Channel {
    tx: Sender<([u8; 1500], usize)>,
    parameters: RwLock<ChannelParameters>,
    mac: MacAddress,
    tun: Arc<Tun>,
    from: String,
    to: String,
}

impl Channel {
    pub fn params(&self) -> ChannelParameters {
        *self.parameters.read().unwrap()
    }

    pub fn set_params(&self, params: HashMap<String, String>) -> Result<()> {
        let result = ChannelParameters {
            latency: Duration::from_millis(u64::from_str_radix(
                params.get("latency").context("could not get latency")?,
                10,
            )?),
            loss: f64::from_str(params.get("loss").context("could not get loss")?)?,
        };

        let mut inner_params = self.parameters.write().unwrap();
        *inner_params = result;
        Ok(())
    }

    pub fn new(
        parameters: ChannelParameters,
        mac: MacAddress,
        tun: Arc<Tun>,
        from: String,
        to: String,
    ) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
        tracing::info!(from, to, ?parameters, "Created channel");
        let this = Arc::new(Self {
            tx,
            parameters: parameters.into(),
            mac,
            tun,
            from,
            to,
        });
        let thisc = this.clone();
        tokio::spawn(async move {
            loop {
                let Some((buf, size)) = rx.recv().await else {
                    continue;
                };
                thisc.priv_send(buf, size).await;
            }
        });
        this
    }

    pub async fn send(&self, buf: [u8; 1500], size: usize) {
        if !self.parameters.read().unwrap().latency.is_zero() {
            let _ = self.tx.send((buf, size)).await;
            return;
        }

        async move {
            match self.should_send(&buf[..size]) {
                Ok(()) => {
                    let _ = self.tun.send_all(&buf[..size]).await;
                }
                Err(e) => {
                    tracing::trace!(self.from, self.to, ?e, "not sent");
                }
            }
        }
        .await;
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

    async fn priv_send(&self, buf: [u8; 1500], size: usize) {
        async move {
            match self.should_send(&buf[..size]) {
                Ok(()) => {
                    let latency = self.parameters.read().unwrap().latency;
                    let tun = self.tun.clone();
                    tokio::spawn(async move {
                        let _ = tokio_timerfd::sleep(latency).await;
                        let _ = tun.send_all(&buf[..size]).await;
                    });
                }
                Err(e) => {
                    tracing::trace!(?e, "not sent");
                }
            }
        }
        .await;
    }
}

pub struct Simulator {
    _namespaces: Vec<NamespaceWrapper>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
}

impl Simulator {
    fn parse_topology(
        config_file: &str,
        callback: impl Fn(&str, &HashMap<String, Value>) -> Result<(Arc<Device>, Arc<Tun>)> + Clone,
    ) -> Result<(
        HashMap<String, HashMap<String, Arc<Channel>>>,
        Vec<NamespaceWrapper>,
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
            (HashMap::default(), Vec::default()),
            |(channels, mut namespaces), (node, node_params)| {
                let Ok(device) =
                    Self::create_namespaces(&mut namespaces, node, node_params, callback.clone())
                else {
                    return (channels, namespaces);
                };

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
                                    device.0.mac_address,
                                    device.1.clone(),
                                    tnode.clone(),
                                    node.to_string(),
                                ),
                            );
                            channels
                        }),
                    namespaces,
                )
            },
        ))
    }

    fn create_namespaces(
        ns_list: &mut Vec<NamespaceWrapper>,
        node: &str,
        node_type: &HashMap<String, Value>,
        callback: impl Fn(&str, &HashMap<String, Value>) -> Result<(Arc<Device>, Arc<Tun>)>,
    ) -> Result<(Arc<Device>, Arc<Tun>)> {
        let node_name = format!("sim_ns_{node}");
        let ns = NamespaceWrapper::new(NetNs::new(node_name.clone())?);
        let Some(nsi) = ns.0.as_ref() else {
            bail!("no namespace");
        };
        let Ok(Ok(device)) = nsi.run(|_| callback(&node, node_type)) else {
            bail!("error creating namespace");
        };
        ns_list.push(ns);
        Ok(device)
    }

    pub fn new<F>(args: &SimArgs, callback: F) -> Result<Self>
    where
        F: Fn(&str, &HashMap<String, Value>) -> Result<(Arc<Device>, Arc<Tun>)> + Clone,
    {
        let (channels, namespaces) = Self::parse_topology(&args.config_file, callback)?;
        Ok(Self {
            _namespaces: namespaces,
            channels,
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

        loop {
            if let Some(Ok((buf, size, node, channel))) = future_set.next().await {
                if let Some(connections) = self.channels.get(&node) {
                    for channel in connections.values() {
                        channel.send(buf, size).await;
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
        let buf = uninit_array![u8; 1500];
        let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
        let n = channel.recv(&mut buf).await?;
        Ok((buf, n, node, channel))
    }

    pub fn get_channels(&self) -> HashMap<String, HashMap<String, Arc<Channel>>> {
        self.channels.clone()
    }
}
