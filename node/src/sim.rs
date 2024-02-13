use std::{collections::HashMap, net::Ipv4Addr, str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Error, Result};
use clap::Parser;
use config::Config;
use futures::{stream::FuturesUnordered, StreamExt};
use mac_address::MacAddress;
use netns_rs::NetNs;
use node::{args::Args, dev::Device};
use tokio::signal;
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

#[derive(Debug)]
struct Parameters {
    latency: Duration,
    loss: f64,
}

struct Topology {
    map: HashMap<Uuid, (String, MacAddress, Arc<Tun>)>,
    topology: HashMap<String, HashMap<String, Parameters>>,
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

    let topology: HashMap<String, HashMap<String, Parameters>> = topology
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

                        (
                            onode.clone(),
                            Parameters {
                                latency: Duration::from_millis(latency),
                                loss,
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect();

    nodes.iter().fold(1, |acc, node| {
        let ns = match NetNs::new(format!("sim_ns_{node}")) {
            Ok(ns) => ns,
            _ => return acc,
        };
        match ns.run(|_| {
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
                uuid: Some(loop {
                    let uuid = Uuid::new_v4();
                    if map.contains_key(&uuid) {
                        continue;
                    }

                    let mac_address =
                        mac_address::mac_address_by_name(tun.name())?.context("mac")?;

                    map.insert(uuid.clone(), (node.clone(), mac_address, tun.clone()));
                    break uuid;
                }),
                ip: Some(Ipv4Addr::from_str(&format!("10.0.0.{}", acc)).expect("Valid IP")),
            };

            let vtun = Arc::new(
                Tun::builder()
                    .tap(true)
                    .packet_info(false)
                    .address(args.ip.context("")?)
                    .mtu(1440)
                    .up()
                    .try_build()?,
            );

            let tuple = Device::new(&args.bind)?;
            tokio::spawn(node::create_with_vdev(args, vtun, tuple));
            Ok::<(), Error>(())
        }) {
            Ok(_) => (),
            _ => return acc,
        };

        namespaces.push(ns);
        acc + 1
    });

    Ok((Topology { map, topology }, namespaces))
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
            for (_, (node, _, tun)) in &topology.map {
                let tun = tun.clone();
                futureset.push(async move {
                    let buf = uninit_array![u8; 1500];
                    let mut buf = buf
                        .iter()
                        .take(1500)
                        .map(|mu| unsafe { mu.assume_init() })
                        .collect::<Vec<_>>();
                    let n = tun.recv(&mut buf).await?;
                    Ok::<(Vec<u8>, usize, std::string::String), Error>((buf, n, node.clone()))
                });
            }

            if let Some(Ok((buf, n, node))) = futureset.next().await {
                for (_, (onode, omac_address, otun)) in &topology.map {
                    if let Some(topology) = topology.topology.get(&node) {
                        if let Some(parameters) = topology.get(onode) {
                            if buf[0..6] != vec![255; 6] && buf[0..6] != omac_address.bytes() {
                                continue;
                            }

                            {
                                let mut rng = rand::thread_rng();
                                if rand::Rng::gen::<f64>(&mut rng) < parameters.loss {
                                    continue;
                                }
                            }

                            if !parameters.latency.is_zero() {
                                tokio::time::sleep(parameters.latency).await;
                            }
                            let _ = otun.send_all(&buf[..n]).await;
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
