use anyhow::Context;
use anyhow::{bail, Result};
use clap::Parser;
use clap::ValueEnum;
use config::Config;
use node_lib::control::args::NodeType;
use node_lib::{control::args::Args, dev::Device};
use std::str::FromStr;
use std::{collections::HashMap, net::Ipv4Addr, sync::Arc};
use tokio::signal;
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod sim_args;
use sim_args::SimArgs;

mod simulator;
use simulator::{Channel, ChannelStats, Simulator};

async fn get_topology(
    topology: Arc<HashMap<String, HashMap<String, Arc<Channel>>>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let result: Vec<(_, _)> = topology
        .iter()
        .flat_map(|(_, y)| {
            y.iter()
                .map(move |(on, c)| (on, c.stats.read().unwrap().clone()))
        })
        .collect();
    let result = result.iter().fold(
        HashMap::default(),
        |mut map: HashMap<String, ChannelStats>, (node, stats)| {
            map.entry((**node).to_string())
                .and_modify(|stored_stats: &mut ChannelStats| *stored_stats += *stats)
                .or_insert(*stats);
            map
        },
    );
    Ok(warp::reply::json(&result))
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

    let simulator = Simulator::new(args.clone(), |config| {
        tracing::info!(?config, "building a node");
        let Some(config) = config.get("config_path") else {
            bail!("no config for node");
        };

        let config = config.to_string();

        let settings = Config::builder()
            .add_source(config::File::with_name(&config))
            .build()?;
        tracing::info!(?settings, "settings");

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
            ip: Some(Ipv4Addr::from_str(&settings.get_string("ip")?)?),
            mtu: 1459,
            node_params: node_lib::control::args::NodeParameters {
                node_type: NodeType::from_str(&settings.get_string("node_type")?, true)
                    .or_else(|_| bail!("invalid node type"))?,
                hello_history: settings.get_int("hello_history")?.try_into()?,
                hello_periodicity: settings.get_int("hello_periodicity").map(|x| x as u32).ok(),
            },
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

        let dev = Arc::new(Device::new(&tun.name())?);
        tokio::spawn(node_lib::create_with_vdev(args, virtual_tun, dev.clone()));
        Ok((dev, tun))
    })?;

    tokio::select! {
        _ = simulator.run() => {}
        _ = signal::ctrl_c() => {}
    }
    Ok(())
}
