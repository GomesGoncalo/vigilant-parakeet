use std::{collections::HashMap, net::Ipv4Addr, str::FromStr, sync::Arc};

use anyhow::Result;
use clap::Parser;
use futures::{stream::FuturesUnordered, StreamExt};
use netns_rs::NetNs;
use node::{args::Args, dev::Device};
use tokio::signal;
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uninit::uninit_array;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct SimArgs {
    /// Network Args
    #[arg(short, long)]
    pub num_nodes: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let mut map = HashMap::new();
    let mut namespaces = Vec::new();

    let args = SimArgs::parse();

    for i in 0..args.num_nodes.unwrap() {
        let ns = NetNs::new(format!("sim_ns_{i}")).unwrap();
        ns.run(|_| {
            let tun = Arc::new(
                Tun::builder()
                    .name("tapsim%d")
                    .tap(true)
                    .packet_info(false)
                    .up()
                    .try_build()
                    .unwrap(),
            );
            let args = Args {
                bind: tun.name().to_string(),
                tap_name: Some(format!("vtap{i}")),
                uuid: Some(loop {
                    let uuid = Uuid::new_v4();
                    if map.contains_key(&uuid) {
                        continue;
                    }

                    map.insert(uuid.clone(), tun.clone());
                    break uuid;
                }),
                ip: Some(Ipv4Addr::from_str(&format!("10.0.0.{}", i + 1)).expect("Valid IP")),
            };

            let vtun = Arc::new(
                Tun::builder()
                    .name(&args.tap_name.as_ref().unwrap())
                    .tap(true)
                    .packet_info(false)
                    .address(args.ip.unwrap())
                    .mtu(1440)
                    .up()
                    .try_build()
                    .unwrap(),
            );

            let tuple = Device::new(&args.bind).expect("created the device");
            tokio::spawn(node::create_with_vdev(args, vtun, tuple));
        })
        .unwrap();

        namespaces.push(ns);
    }

    tokio::spawn(async move {
        loop {
            let mut futureset = FuturesUnordered::new();
            for (uuid, tun) in &map {
                let tun = tun.clone();
                futureset.push(async move {
                    let buf = uninit_array![u8; 1500];
                    let mut buf = buf
                        .iter()
                        .take(1500)
                        .map(|mu| unsafe { mu.assume_init() })
                        .collect::<Vec<_>>();
                    let n = tun.recv(&mut buf).await.unwrap();
                    (buf, uuid, n)
                });
            }

            if let Some((buf, uuid, n)) = futureset.next().await {
                for (ouuid, otun) in &map {
                    if uuid == ouuid {
                        continue;
                    }
                    let _ = otun.send_all(&buf[..n]).await;
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
