use anyhow::{bail, Context, Error, Result};
use clap::Parser;
use clap::ValueEnum;
use config::{Config, Value};
use futures::{stream::FuturesUnordered, StreamExt};
use mac_address::MacAddress;
use netns_rs::NetNs;
use node::{
    control::args::{Args, NodeParameters, NodeType},
    dev::Device,
};
use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::{signal, sync::mpsc::Sender};
use tokio_tun::Tun;
use tracing::Instrument;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uninit::uninit_array;

struct NamespaceWrapper(Option<NetNs>);

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

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,

    #[arg(short, long, default_value_t = false)]
    pub pretty: bool,
}

#[derive(Debug, Clone)]
struct SimNodeParameters(NodeParameters);
impl TryFrom<HashMap<String, Value>> for SimNodeParameters {
    type Error = anyhow::Error;
    fn try_from(param: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let node_type = &param
            .get("node_type")
            .context("no node type")?
            .clone()
            .into_string()?;

        let Ok(node_type) = NodeType::from_str(node_type, true) else {
            bail!("invalid node type");
        };

        let hello_history = u32::try_from(
            param
                .get("hello_history")
                .context("hello history")?
                .clone()
                .into_uint()?,
        )?;

        let hello_periodicity = match param.get("hello_periodicity") {
            Some(v) => match v.clone().into_uint() {
                Ok(v) => Some(u32::try_from(v)?),
                Err(_) => None,
            },
            None => None,
        };

        Ok(Self(NodeParameters {
            node_type,
            hello_history,
            hello_periodicity,
        }))
    }
}

#[derive(Debug, Copy, Clone)]
struct ChannelParameters {
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

struct Channel {
    tx: Sender<Arc<[u8]>>,
    parameters: ChannelParameters,
    mac: MacAddress,
    tun: Arc<Tun>,
    from: String,
    to: String,
}

impl Channel {
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
            parameters,
            mac,
            tun,
            from,
            to,
        });
        let thisc = this.clone();
        tokio::spawn(
            async move {
                loop {
                    let Some(buf) = rx.recv().await else {
                        continue;
                    };
                    thisc.priv_send(buf).await;
                }
            }
            .in_current_span(),
        );
        this
    }

    pub async fn send(&self, buf: &Arc<[u8]>) {
        if !self.parameters.latency.is_zero() {
            let _ = self.tx.send(buf.clone()).await;
            return;
        }

        async move {
            match self.should_send(buf) {
                Ok(buf) => {
                    let _ = self.tun.send_all(buf).await;
                }
                Err(e) => {
                    tracing::trace!(self.from, self.to, ?e, "not sent");
                }
            }
        }
        .await;
    }

    fn should_send<'a>(&self, buf: &'a Arc<[u8]>) -> Result<&'a Arc<[u8]>> {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            bail!("not the right mac address")
        }

        if self.parameters.loss > 0.0 {
            let mut rng = rand::thread_rng();
            if rand::Rng::gen::<f64>(&mut rng) < self.parameters.loss {
                bail!("packet lost")
            }
        }

        Ok(buf)
    }

    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let n = self.tun.recv(buf).await?;
        Ok(n)
    }

    async fn priv_send(&self, buf: Arc<[u8]>) {
        async move {
            match self.should_send(&buf) {
                Ok(buf) => {
                    let latency = self.parameters.latency;
                    let tun = self.tun.clone();
                    let buf = buf.clone();
                    tokio::spawn(async move {
                        let _ = tokio_timerfd::sleep(latency).await;
                        let _ = tun.send_all(&buf).await;
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

fn create_namespaces(
    ns_list: &mut Vec<NamespaceWrapper>,
    node: &str,
    node_type: &SimNodeParameters,
) -> Result<(Arc<Device>, Arc<Tun>)> {
    let node_name = format!("sim_ns_{node}");
    let ns = NetNs::new(node_name.clone())?;
    let ns_result = ns.run(|_| {
        let tun = Arc::new(
            Tun::builder()
                .name("real")
                .tap(true)
                .packet_info(false)
                .up()
                .try_build()?,
        );

        let args = Args {
            bind: tun.name().to_string(),
            tap_name: Some("virtual".to_string()),
            ip: Some(Ipv4Addr::from_str(&format!(
                "10.0.0.{}",
                ns_list.len() + 1
            ))?),
            mtu: 1459,
            node_params: node_type.0.clone(),
        };

        let virtual_tun = if let Some(ref name) = args.tap_name {
            Arc::new(
                Tun::builder()
                    .tap(true)
                    .name(name)
                    .packet_info(false)
                    .address(args.ip.context("")?)
                    .mtu(args.mtu)
                    .up()
                    .try_build()?,
            )
        } else {
            Arc::new(
                Tun::builder()
                    .tap(true)
                    .packet_info(false)
                    .address(args.ip.context("")?)
                    .mtu(args.mtu)
                    .up()
                    .try_build()?,
            )
        };

        let dev = Arc::new(Device::new(&args.bind)?);
        let span = tracing::debug_span!("sim node", node = node_name, mac = %dev.mac_address);
        let _guard = span.enter();
        tokio::spawn(node::create_with_vdev(args, virtual_tun, dev.clone()).in_current_span());
        Ok::<_, Error>((dev, tun))
    });

    let Ok(Ok(device)) = ns_result else {
        bail!("error creating namespace");
    };

    ns_list.push(NamespaceWrapper::new(ns));
    Ok(device)
}

fn parse_topology(
    config_file: &str,
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
            let Ok(param) = SimNodeParameters::try_from(param) else {
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
            let Ok(device) = create_namespaces(&mut namespaces, node, node_params) else {
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

async fn generate_channel_reads(
    node: String,
    channel: Arc<Channel>,
) -> Result<(Arc<[u8]>, String, Arc<Channel>), Error> {
    let buf = uninit_array![u8; 1500];
    let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
    let n = channel.recv(&mut buf).await?;
    Ok((buf[..n].into(), node, channel))
}

async fn run(topology: HashMap<String, HashMap<String, Arc<Channel>>>) -> Result<()> {
    let mut set = HashSet::new();
    let mut future_set = topology
        .values()
        .flat_map(|x| x.iter())
        .filter_map(|(node, channel)| {
            if !set.insert(node) {
                return None;
            }

            Some(generate_channel_reads(node.to_string(), channel.clone()))
        })
        .collect::<FuturesUnordered<_>>();
    std::mem::drop(set);

    loop {
        if let Some(Ok((buf, node, channel))) = future_set.next().await {
            if let Some(connections) = topology.get(&node) {
                for channel in connections.values() {
                    channel.send(&buf).await;
                }
            }

            future_set.push(generate_channel_reads(node, channel));
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = SimArgs::parse();

    if args.pretty {
        tracing_subscriber::registry()
            .with(fmt::layer().with_thread_ids(true).pretty())
            .with(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer().with_thread_ids(true))
            .with(EnvFilter::from_default_env())
            .init();
    }

    // Namespaces must be kept alive till the end, we do not want to run destructor
    // Same logic as the span guards
    let (topology, _namespaces) = match parse_topology(&args.config_file) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(?e, "Error parsing topology");
            bail!(e)
        }
    };

    tokio::select! {
        _ = run(topology) => {}
        _ = signal::ctrl_c() => {}
    }
    Ok(())
}
