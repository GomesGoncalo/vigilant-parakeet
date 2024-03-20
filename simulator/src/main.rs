use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use common::device::Device;
use config::Config;
#[cfg(feature = "webview")]
use itertools::Itertools;
use node_lib::args::{Args, NodeParameters, NodeType};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::signal;
use tokio_tun::Tun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
#[cfg(feature = "webview")]
use warp::Filter;

mod sim_args;
use sim_args::SimArgs;

mod simulator;
use simulator::{Channel, Simulator};

#[cfg(feature = "webview")]
async fn channel_post_fn(
    src: String,
    dst: String,
    post: HashMap<String, String>,
    channels: HashMap<String, HashMap<String, Arc<Channel>>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    use warp::reply::Reply;

    let Some(src1) = channels.get(&src) else {
        return Err(warp::reject::not_found());
    };

    let Some(channel) = src1.get(&dst) else {
        return Err(warp::reject::not_found());
    };

    if channel.set_params(post).is_ok() {
        Ok(warp::reply().into_response())
    } else {
        Ok(
            warp::reply::with_status(warp::reply(), warp::http::StatusCode::BAD_REQUEST)
                .into_response(),
        )
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

    let devices = Arc::new(Mutex::new(HashMap::new()));
    let simulator = Simulator::new(&args, |name, config| {
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
            node_params: NodeParameters {
                node_type: NodeType::from_str(&settings.get_string("node_type")?, true)
                    .or_else(|_| bail!("invalid node type"))?,
                hello_history: settings.get_int("hello_history")?.try_into()?,
                hello_periodicity: settings
                    .get_int("hello_periodicity")
                    .map(|x| u32::try_from(x).ok())
                    .ok()
                    .flatten(),
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

        let dev = Arc::new(Device::new(tun.name())?);
        tokio::spawn(node_lib::create_with_vdev(args, virtual_tun, dev.clone()));
        devices
            .lock()
            .unwrap()
            .insert(name.to_string(), dev.clone());
        Ok((dev, tun))
    })?;

    #[cfg(feature = "webview")]
    {
        let devicesc = devices.clone();
        let nodes = warp::get()
            .and(warp::path("nodes"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(&devicesc.lock().unwrap().keys().cloned().collect_vec())
            });

        let stats = warp::get()
            .and(warp::path("stats"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(
                    &devices
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(node, device)| (node, device.stats()))
                        .collect::<HashMap<_, _>>(),
                )
            });

        let channels = simulator.get_channels();
        let channelsc = channels.clone();
        let channels_get = warp::get()
            .and(warp::path("channels"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(
                    &channelsc
                        .iter()
                        .map(|(node, onode)| {
                            (
                                node,
                                onode
                                    .iter()
                                    .map(|(onode, channel)| (onode, channel.params()))
                                    .collect::<HashMap<_, _>>(),
                            )
                        })
                        .collect::<HashMap<_, _>>(),
                )
            });

        let channelsc = channels.clone();
        let channel_post = warp::post()
            .and(warp::path!("channel" / String / String))
            .and(warp::path::end())
            .and(warp::body::json())
            .and_then(move |src, dst, post| channel_post_fn(src, dst, post, channelsc.clone()));

        let routes = nodes.or(stats).or(channels_get).or(channel_post);
        tokio::select! {
            _ = warp::serve(routes).run(([127, 0, 0, 1], 3030)) => {}
            _ = simulator.run() => {}
            _ = signal::ctrl_c() => {}
        }
    }
    #[cfg(not(feature = "webview"))]
    {
        tokio::select! {
            _ = simulator.run() => {}
            _ = signal::ctrl_c() => {}
        }
    }

    Ok(())
}
