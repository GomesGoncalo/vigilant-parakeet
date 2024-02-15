use std::{collections::HashMap, net::Ipv4Addr, str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Error, Result};
use clap::Parser;
use config::Config;
use futures::{stream::FuturesUnordered, StreamExt};
use mac_address::MacAddress;
use netns_rs::NetNs;
use node::{args::Args, dev::Device};
use tokio::{signal, sync::mpsc::Sender};
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uninit::uninit_array;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct SimArgs {
    /// Topology configuration
    #[arg(short, long)]
    pub config_file: String,
}

struct Channel {
    tx: Sender<Arc<Vec<u8>>>,
    latency: Duration,
    loss: f64,
    mac: MacAddress,
    tun: Arc<Tun>,
}

impl Channel {
    pub fn new(latency: Duration, loss: f64, mac: MacAddress, tun: Arc<Tun>) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
        let this = Arc::new(Self {
            tx,
            latency,
            loss,
            mac,
            tun,
        });
        let thisc = this.clone();
        tokio::spawn(async move {
            loop {
                let buf: Arc<Vec<u8>> = match rx.recv().await {
                    Some(buf) => buf,
                    None => continue,
                };

                if thisc.should_send(&buf) {
                    tokio::time::sleep(thisc.latency).await;

                    let _ = thisc.tun.send_all(&buf).await;
                }
            }
        });
        this
    }

    pub async fn send(&self, buf: Arc<Vec<u8>>) {
        if !self.latency.is_zero() {
            let _ = self.tx.send(buf).await;
        } else if self.should_send(&buf) {
            let _ = self.tun.send_all(&buf).await;
        }
    }

    fn should_send(&self, buf: &Arc<Vec<u8>>) -> bool {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            return false;
        }

        {
            let mut rng = rand::thread_rng();
            if rand::Rng::gen::<f64>(&mut rng) < self.loss {
                return false;
            }
        }

        true
    }
}

struct Topology {
    map: HashMap<Uuid, (String, Arc<Tun>)>,
    topology: HashMap<String, HashMap<String, Arc<Channel>>>,
}

fn parse_topology(config_file: &str) -> Result<(Topology, Vec<NetNs>)> {
    let settings = Config::builder()
        .add_source(config::File::with_name(config_file))
        .build()?;

    let topology = settings.get_table("topology")?;
    let nodes = topology
        .get("nodes")
        .context("Nodes defined")?
        .clone()
        .into_array()?
        .iter()
        .map(|val| val.clone().into_string().unwrap_or_default())
        .collect::<Vec<_>>();

    let mut namespaces = Vec::with_capacity(nodes.len());
    let mut map = HashMap::with_capacity(nodes.len());

    let topology: HashMap<String, HashMap<String, (u64, f64)>> = topology
        .iter()
        .map(|(key, val)| {
            let val = val.clone().into_table().unwrap_or_default();
            (
                key.clone(),
                val.iter()
                    .map(|(onode, param)| {
                        let param = param.clone().into_table().unwrap_or_default();
                        let latency = match param.get("latency") {
                            Some(val) => val.clone().into_uint().unwrap_or(0),
                            None => 0,
                        };
                        let loss = match param.get("loss") {
                            Some(val) => val.clone().into_float().unwrap_or(0.0),
                            None => 0.0,
                        };

                        (onode.clone(), (latency, loss))
                    })
                    .collect(),
            )
        })
        .collect();

    let mut topology_channel = HashMap::new();

    nodes.iter().fold(1, |acc, node| {
        let Ok(ns) = NetNs::new(format!("sim_ns_{node}")) else {
            return acc;
        };

        let ns_result = ns.run(|_| {
            let tun = Arc::new(
                Tun::builder()
                    .name("tapsim%d")
                    .tap(true)
                    .packet_info(false)
                    .up()
                    .try_build()?,
            );

            let mac_address = mac_address::mac_address_by_name(tun.name())?.context("mac")?;
            for (tnode, connections) in &topology {
                for (onode, (latency, loss)) in connections {
                    if onode != node {
                        continue;
                    }
                    if !topology_channel.contains_key(tnode) {
                        topology_channel.insert(tnode.clone(), HashMap::new());
                    }

                    // SAFETY: We just inserted it, it must have the key
                    let connection = topology_channel.get_mut(tnode).unwrap();
                    connection.insert(
                        onode.clone(),
                        Channel::new(
                            Duration::from_millis(*latency),
                            *loss,
                            mac_address,
                            tun.clone(),
                        ),
                    );
                    break;
                }
            }

            let args = Args {
                bind: tun.name().to_string(),
                tap_name: None,
                uuid: Some(loop {
                    let uuid = Uuid::new_v4();
                    if map.contains_key(&uuid) {
                        continue;
                    }

                    map.insert(uuid, (node.clone(), tun.clone()));
                    break uuid;
                }),
                ip: Some(Ipv4Addr::from_str(&format!("10.0.0.{acc}")).expect("Valid IP")),
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

            let tuple = Device::new(&args.bind)?;
            tokio::spawn(node::create_with_vdev(args, virtual_tun, tuple));
            Ok::<(), Error>(())
        });

        match ns_result {
            Ok(Ok(())) => (),
            _ => return acc,
        };

        namespaces.push(ns);
        acc + 1
    });

    Ok((
        Topology {
            map,
            topology: topology_channel,
        },
        namespaces,
    ))
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
        loop {
            let mut futureset = FuturesUnordered::new();
            for (node, tun) in topology.map.values() {
                let tun = tun.clone();
                futureset.push(async move {
                    let buf = uninit_array![u8; 1500];
                    let mut buf = buf
                        .iter()
                        .take(1500)
                        .map(|mu| unsafe { mu.assume_init() })
                        .collect::<Vec<_>>();
                    let n = tun.recv(&mut buf).await?;
                    buf.truncate(n);
                    Ok::<(Vec<u8>, String), Error>((buf, node.clone()))
                });
            }

            if let Some(Ok((buf, node))) = futureset.next().await {
                let buf = Arc::new(buf);
                for (onode, _) in topology.map.values() {
                    if let Some(topology) = topology.topology.get(&node) {
                        if let Some(channel) = topology.get(onode) {
                            channel.send(buf.clone()).await;
                        }
                    }
                }
            }
        }
    });

    signal::ctrl_c().await.expect("failed to listen for event");
    for ns in namespaces {
        let _ = ns.remove();
    }
    Ok(())
}
