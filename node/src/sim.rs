use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use clap::Parser;
use futures::{stream::FuturesUnordered, StreamExt};
use node::args::Args;
use tokio::sync::RwLock;
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
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

    let map = Arc::new(RwLock::new(HashMap::new()));

    let args = SimArgs::parse();

    for _ in 0..args.num_nodes.unwrap() {
        let tun = Arc::new(
            Tun::builder()
                .name("tapsim%d")
                .tap(true)
                .packet_info(false)
                .up()
                .try_build()
                .unwrap(),
        );

        tokio::spawn(node::create(Args {
            bind: tun.name().to_string(),
            tap_name: None,
            uuid: Some(loop {
                let uuid = Uuid::new_v4();
                let mut map = map.write().await;
                if map.contains_key(&uuid) {
                    continue;
                }

                map.insert(uuid.clone(), tun.clone());
                break uuid;
            }),
        }));
    }

    loop {
        let mut futureset = FuturesUnordered::new();
        for (_, tun) in map.read().await.iter() {
            let tun = tun.clone();
            futureset.push(async move {
                let mut buf = [0; 1500];
                let n = tun.recv(&mut buf).await.unwrap();
                (buf[..n].to_vec(), tun.clone())
            });
        }

        let map = map.clone();
        if let Some((buf, tun)) = futureset.next().await {
            let span = tracing::trace_span!("sending packet", from = tun.name());
            let _enter = span.enter();
            for (_, otun) in map.read().await.iter() {
                if tun.name() == otun.name() {
                    tracing::trace!(iface = otun.name(), "Ignore same interface");
                    continue;
                }
                let _ = otun.send_all(&buf).await;
                tracing::trace!(to = otun.name(), "Distributed packet")
            }
        }
    }
}
