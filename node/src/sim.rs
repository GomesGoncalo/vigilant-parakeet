use std::{collections::HashMap, future, sync::Arc};

use anyhow::Result;
use futures::{stream::FuturesUnordered, StreamExt};
use node::{args::Args, create};
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().with_thread_ids(true).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let mut map = HashMap::new();

    for _ in 0..2 {
        let tun = Arc::new(
            Tun::builder()
                .name("tapsim%d")
                .tap(true)
                .packet_info(false)
                .up()
                .try_build()
                .unwrap(),
        );

        let uuid = loop {
            let uuid = Uuid::new_v4();
            if map.contains_key(&uuid) {
                continue;
            }

            map.insert(uuid.clone(), tun.clone());
            break uuid;
        };

        let args = Args {
            bind: tun.name().to_string(),
            tap_name: None,
            uuid: Some(uuid),
        };

        tokio::spawn(create(args));
    }

    loop {
        let mut futureset = FuturesUnordered::new();
        for (_, tun) in &map {
            let tun = tun.clone();
            futureset.push(async move {
                let mut buf = [0; 1500];
                let n = tun.recv(&mut buf).await.unwrap();
                (buf[..n].to_vec(), tun.clone())
            });
        }

        if let Some((buf, tun)) = futureset.next().await {
            for (_, otun) in &map {
                if tun.name() == otun.name() {
                    continue;
                }
                let _ = otun.send_all(&buf).await;
            }
        }
    }
}
