use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use common::device::Device;
use common::tun::Tun;
use config::Config;
#[cfg(feature = "webview")]
use itertools::Itertools;
use node_lib::args::{Args, NodeParameters, NodeType};
use serde::Serialize;
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::signal;
use tokio_tun::Tun as TokioTun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
#[cfg(feature = "webview")]
use warp::Filter;

mod sim_args;
use sim_args::SimArgs;

mod simulator;
use simulator::Simulator;

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

#[allow(dead_code)]
#[derive(Serialize)]
struct ErrorMessage {
    code: u16,
    message: String,
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

        let tun = Arc::new(Tun::new(
            TokioTun::builder()
                .name("real")
                .tap()
                .up()
                .build()?
                .into_iter()
                .next()
                .expect("Expecting at least 1 item in vec"),
        ));

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

        let virtual_tun = Arc::new(Tun::new(if let Some(ref name) = args.tap_name {
            TokioTun::builder()
                .tap()
                .name(name)
                .address(args.ip.context("")?)
                .mtu(args.mtu)
                .up()
                .build()?
                .into_iter()
                .next()
                .expect("Expecting at least 1 item in vec")
        } else {
            TokioTun::builder()
                .tap()
                .address(args.ip.context("")?)
                .mtu(args.mtu)
                .up()
                .build()?
                .into_iter()
                .next()
                .expect("Expecting at least 1 item in vec")
        }));

        let dev = Arc::new(Device::new(tun.name())?);
        let node = node_lib::create_with_vdev(args, virtual_tun, dev.clone())?;
        devices
            .lock()
            .unwrap()
            .insert(name.to_string(), (dev.clone(), tun.clone()));
        Ok((dev, tun, node))
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

        let devicesc = devices.clone();
        let stats = warp::get()
            .and(warp::path("stats"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(
                    &devicesc
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(node, (device, tun))| (node, (device.stats(), tun.stats())))
                        .collect::<HashMap<_, _>>(),
                )
            });

        let node_stats = warp::get()
            .and(warp::path!("node" / String))
            .and(warp::path::end())
            .map(move |node: String| {
                let Some(reply) = &devices
                    .lock()
                    .unwrap()
                    .get(&node)
                    .map(|(device, tun)| (device.stats(), tun.stats()))
                else {
                    let json = warp::reply::json(&ErrorMessage {
                        code: 404,
                        message: "node not found".to_string(),
                    });
                    return warp::reply::with_status(json, warp::http::StatusCode::NOT_FOUND);
                };

                let mut map = HashMap::with_capacity(1);
                map.insert(node, *reply);
                let json = warp::reply::json(&map);
                warp::reply::with_status(json, warp::http::StatusCode::OK)
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

        let cors = warp::cors().allow_any_origin();

        let routes = nodes
            .or(node_stats)
            .or(stats)
            .or(channels_get)
            .or(channel_post)
            .with(cors);
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
