use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Error, Result};
use clap::Parser;
use config::{Config, Value};
use futures::{stream::FuturesUnordered, StreamExt};
use mac_address::MacAddress;
use netns_rs::NetNs;
use node::{args::Args, dev::Device};
use tokio::{signal, sync::mpsc::Sender};
use tokio_tun::Tun;
use tracing::Instrument;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uninit::uninit_array;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,
}

#[derive(Debug, Copy, Clone)]
struct Parameters {
    latency: Duration,
    loss: f64,
}

impl From<HashMap<String, Value>> for Parameters {
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
    parameters: Parameters,
    mac: MacAddress,
    tun: Arc<Tun>,
    from: String,
    to: String,
}

impl Channel {
    pub fn new(
        parameters: Parameters,
        mac: MacAddress,
        tun: Arc<Tun>,
        from: String,
        to: String,
    ) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(100000);
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
        tokio::spawn(async move {
            loop {
                let buf: Arc<[u8]> = match rx.recv().await {
                    Some(buf) => buf,
                    None => continue,
                };
                thisc.priv_send(buf).await;
            }
        });
        this
    }

    pub async fn send(&self, buf: &Arc<[u8]>) {
        if !self.parameters.latency.is_zero() {
            let _ = self.tx.send(buf.clone()).await;
            return;
        }

        let span = tracing::trace_span!(target: "pkt", "send", ?buf);
        let span1 = tracing::trace_span!("targets", self.from, self.to);
        async move {
            match self.should_send(buf) {
                Ok(buf) => {
                    let _ = self.tun.send_all(&buf).await;
                    tracing::trace!(self.from, self.to, "sent a packet");
                }
                Err(e) => {
                    tracing::trace!(self.from, self.to, ?e, "not sent");
                }
            }
        }
        .instrument(span)
        .instrument(span1)
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

        Ok(&buf)
    }

    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let n = self.tun.recv(buf).await?;
        let span = tracing::trace_span!(target: "pkt", "recv", buf = ?buf[..n]);
        let span1 = tracing::trace_span!("targets", from = self.to);
        let _guard = span.enter();
        let _guard1 = span1.enter();
        tracing::trace!("recv a packet");
        Ok(n)
    }

    async fn priv_send(&self, buf: Arc<[u8]>) {
        let span = tracing::trace_span!(target: "pkt", "send", ?buf);
        let span1 = tracing::trace_span!("targets", self.from, self.to);
        let now = Instant::now();
        async move {
            match self.should_send(&buf) {
                Ok(buf) => {
                    tokio::time::sleep(self.parameters.latency).await;
                    let _ = self.tun.send_all(&buf).await;
                    tracing::trace!(delay = ?now.elapsed(), "sent a packet");
                }
                Err(e) => {
                    tracing::trace!(?e, "not sent");
                }
            }
        }
        .instrument(span)
        .instrument(span1)
        .await;
    }
}

fn create_namespaces(ns_list: &mut Vec<NetNs>, node: &str) -> Result<(Arc<Device>, Arc<Tun>)> {
    let ns = NetNs::new(format!("sim_ns_{node}"))?;
    let ns_result = ns.run(|_| {
        let tun = Arc::new(
            Tun::builder()
                .name("tapsim%d")
                .tap(true)
                .packet_info(false)
                .up()
                .try_build()?,
        );

        let args = Args {
            bind: tun.name().to_string(),
            tap_name: None,
            uuid: None,
            ip: Some(
                Ipv4Addr::from_str(&format!("10.0.0.{}", ns_list.len() + 1)).expect("Valid IP"),
            ),
        };

        let virtual_tun = Arc::new(
            Tun::builder()
                .tap(true)
                .packet_info(false)
                .address(args.ip.context("")?)
                .mtu(1440)
                .up()
                .try_build()?,
        );

        let dev = Arc::new(Device::new(&args.bind)?);
        tokio::spawn(node::create_with_vdev(args, virtual_tun, dev.clone()));
        Ok::<_, Error>((dev, tun))
    });

    let Ok(Ok(device)) = ns_result else {
        bail!("error creating namespace");
    };

    ns_list.push(ns);
    Ok(device)
}

fn parse_topology(
    config_file: &str,
) -> Result<(HashMap<String, HashMap<String, Arc<Channel>>>, Vec<NetNs>)> {
    let settings = Config::builder()
        .add_source(config::File::with_name(config_file))
        .build()?;

    let nodes = settings
        .get_array("nodes")
        .context("Nodes defined")?
        .iter()
        .map(|val| val.clone().into_string().unwrap_or_default())
        .collect::<Vec<_>>();

    let topology = settings.get_table("topology")?;
    let topology: HashMap<String, HashMap<String, Parameters>> = topology
        .iter()
        .map(|(key, val)| {
            let val = val.clone().into_table().unwrap_or_default();
            (
                key.clone(),
                val.iter()
                    .map(|(onode, param)| {
                        let param = param.clone().into_table().unwrap_or_default();
                        let param = Parameters::from(param);
                        (onode.clone(), param)
                    })
                    .collect(),
            )
        })
        .collect();

    Ok(nodes.iter().fold(
        (HashMap::default(), Vec::default()),
        |(channels, mut namespaces), node| {
            let Ok(device) = create_namespaces(&mut namespaces, node) else {
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
                            node.clone(),
                            Channel::new(
                                *parameters,
                                device.0.mac_address,
                                device.1.clone(),
                                tnode.clone(),
                                node.clone(),
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let args = SimArgs::parse();

    let (topology, namespaces) = parse_topology(&args.config_file)?;
    tokio::spawn(async move {
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
    });

    signal::ctrl_c().await.expect("failed to listen for event");
    for ns in namespaces {
        let _ = ns.remove();
    }
    Ok(())
}
