#[cfg(feature = "webview")]
use crate::simulator::Channel;
#[cfg(not(feature = "test_helpers"))]
use anyhow::Context;
use anyhow::{bail, Result};
use clap::{Parser, ValueEnum};
use common::device::Device;
#[cfg(feature = "webview")]
use common::network_interface::NetworkInterface;
#[cfg(not(feature = "test_helpers"))]
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
// Context is unused in current builds; remove the import.
#[cfg(not(feature = "test_helpers"))]
use tokio_tun::Tun as TokioTun;
use tracing::info;
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
    let devices_for_closure = devices.clone();
    let server_addr = Some(args.server_address);
    let simulator = Simulator::new(&args, move |name, config| {
        let Some(config) = config.get("config_path") else {
            bail!("no config for node");
        };

        let config = config.to_string();

        let settings = Config::builder()
            .add_source(config::File::with_name(&config))
            .build()?;
        tracing::info!(?settings, "settings");

        #[cfg(not(feature = "test_helpers"))]
        let tun = Arc::new(Tun::new(
            TokioTun::builder()
                .name("real")
                .tap()
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?,
        ));
        #[cfg(feature = "test_helpers")]
        let tun = {
            // test build: use shared test helper to construct a shim Tun and take one end.
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };

        // Read optional cached_candidates; default to 3 when not present or invalid.
        let cached_candidates = settings
            .get_int("cached_candidates")
            .ok()
            .and_then(|x| u32::try_from(x).ok())
            .unwrap_or(3u32);

        let args = Args {
            bind: tun.name().to_string(),
            tap_name: Some("virtual".to_string()),
            ip: Some(Ipv4Addr::from_str(&settings.get_string("ip")?)?),
            mtu: 1436,
            node_params: NodeParameters {
                node_type: NodeType::from_str(&settings.get_string("node_type")?, true)
                    .or_else(|_| bail!("invalid node type"))?,
                hello_history: settings.get_int("hello_history")?.try_into()?,
                hello_periodicity: settings
                    .get_int("hello_periodicity")
                    .map(|x| u32::try_from(x).ok())
                    .ok()
                    .flatten(),
                cached_candidates,
                enable_encryption: settings.get_bool("enable_encryption").unwrap_or(false),
                server_address: settings
                    .get::<std::net::SocketAddr>("server_address")
                    .ok()
                    .or(server_addr), // Use config file setting or command line arg
            },
        };

        #[cfg(not(feature = "test_helpers"))]
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
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
        } else {
            TokioTun::builder()
                .tap()
                .address(args.ip.context("")?)
                .mtu(args.mtu)
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
        }));
        #[cfg(feature = "test_helpers")]
        let virtual_tun = {
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };

        let dev = Arc::new(Device::new(tun.name())?);
        let node = node_lib::create_with_vdev(args, virtual_tun, dev.clone())?;
        devices_for_closure
            .lock()
            .map_err(|e| anyhow::anyhow!("devices mutex poisoned: {}", e))?
            .insert(name.to_string(), (dev.clone(), tun.clone()));
        Ok((dev, tun, node))
    })?;

    // Spawn server (always required now)
    let server_addr = args.server_address;
    info!("Starting server at {}", server_addr);
        
        // Create server TUN device and assign IP address
        let server_ip = Ipv4Addr::new(10, 0, 255, 1); // Use a dedicated server IP
        
        #[cfg(not(feature = "test_helpers"))]
        let server_tun = Arc::new(Tun::new(
            TokioTun::builder()
                .name("server")
                .tap()
                .address(server_ip)
                .mtu(1436)
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder for server"))?,
        ));
        
        #[cfg(feature = "test_helpers")]
        let server_tun = {
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };
        
        let server_device = Arc::new(Device::new(server_tun.name())?);
        
        let _server = node_lib::server::Server::new(server_addr, server_ip, server_tun, server_device).await?;

    #[cfg(feature = "webview")]
    {
        let devicesc = devices.clone();
        let nodes = warp::get()
            .and(warp::path("nodes"))
            .and(warp::path::end())
            .map(move || {
                let guard = devicesc.lock().unwrap_or_else(|e| e.into_inner());
                warp::reply::json(&guard.keys().cloned().collect_vec())
            });

        let devicesc = devices.clone();
        let stats = warp::get()
            .and(warp::path("stats"))
            .and(warp::path::end())
            .map(move || {
                let guard = devicesc.lock().unwrap_or_else(|e| e.into_inner());
                warp::reply::json(
                    &guard
                        .iter()
                        .map(|(node, (device, tun))| (node, (device.stats(), tun.stats())))
                        .collect::<HashMap<_, _>>(),
                )
            });

        let node_stats = warp::get()
            .and(warp::path!("node" / String))
            .and(warp::path::end())
            .map(move |node: String| {
                let guard = devices.lock().unwrap_or_else(|e| e.into_inner());
                let Some(reply) = &guard
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

        // Permit requests from the visualization frontend (CORS) including POST and Content-Type header
        let cors = warp::cors()
            .allow_any_origin()
            .allow_methods(vec!["GET", "POST", "OPTIONS"])
            .allow_headers(vec!["content-type"]);

        // Build a /node_info endpoint returning per-node metadata: { node: { node_type: "Obu"|"Rsu", upstream: Option<{hops, mac}> } }
        let sim_nodes = simulator.get_nodes();
        let sim_nodesc = sim_nodes.clone();
        let node_info = warp::get()
            .and(warp::path("node_info"))
            .and(warp::path::end())
            .map(move || {
                // Build a mapping of MAC -> node name
                let mac_map: HashMap<mac_address::MacAddress, String> = sim_nodesc
                    .iter()
                    .map(|(name, (dev, _tun, _node))| (dev.mac_address(), name.clone()))
                    .collect();

                #[derive(serde::Serialize, Clone)]
                struct UpstreamInfo {
                    hops: u32,
                    mac: String,
                    node_name: Option<String>,
                }

                #[derive(serde::Serialize)]
                struct NodeInfo {
                    node_type: String,
                    upstream: Option<UpstreamInfo>,
                    downstream: Option<Vec<UpstreamInfo>>,
                }

                let mut out: HashMap<String, NodeInfo> = HashMap::new();
                // first pass: compute upstream info per node and stash in a temp map so we can invert for downstream
                let mut upstream_map: HashMap<String, UpstreamInfo> = HashMap::new();
                for (name, (_dev, _tun, node)) in sim_nodes.iter() {
                    // try downcast to obu to get a cached route
                    let node_type = if node.as_any().is::<node_lib::control::obu::Obu>() {
                        "Obu".to_string()
                    } else if node.as_any().is::<node_lib::control::rsu::Rsu>() {
                        "Rsu".to_string()
                    } else {
                        "Unknown".to_string()
                    };
                    let upstream_route = node
                        .as_any()
                        .downcast_ref::<node_lib::control::obu::Obu>()
                        .and_then(|obu| obu.cached_upstream_route())
                        .map(|r| UpstreamInfo {
                            hops: r.hops,
                            mac: format!("{}", r.mac),
                            node_name: mac_map.get(&r.mac).cloned(),
                        });

                    if let Some(ref u) = upstream_route {
                        upstream_map.insert(name.clone(), u.clone());
                    }

                    out.insert(
                        name.clone(),
                        NodeInfo {
                            node_type,
                            upstream: upstream_route,
                            downstream: None,
                        },
                    );
                }

                // second pass: invert upstream_map to produce downstream vectors per node name
                let mut downstream_map: HashMap<String, Vec<UpstreamInfo>> = HashMap::new();
                for (child_name, upinfo) in upstream_map.iter() {
                    if let Some(parent_name) = &upinfo.node_name {
                        let mut di = upinfo.clone();
                        // set the node_name field to the child's name so downstream entry points to that child
                        di.node_name = Some(child_name.clone());
                        downstream_map
                            .entry(parent_name.clone())
                            .or_default()
                            .push(di);
                    }
                }

                // attach downstreams to out
                for (node_name, nd) in downstream_map.into_iter() {
                    if let Some(entry) = out.get_mut(&node_name) {
                        entry.downstream = Some(nd);
                    }
                }
                warp::reply::json(&out)
            });

        let routes = nodes
            .or(node_stats)
            .or(stats)
            .or(channels_get)
            .or(node_info)
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
