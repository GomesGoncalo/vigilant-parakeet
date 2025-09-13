#[cfg(feature = "webview")]
use crate::simulator::Channel;
use anyhow::{bail, Result};
use clap::Parser;
#[cfg(not(feature = "test_helpers"))]
use common::tun::Tun;
#[cfg(feature = "webview")]
use common::network_interface::NetworkInterface;
use config::Config;
#[cfg(feature = "webview")]
use itertools::Itertools;
use node_lib::test_helpers::{util::mk_socketpairs, util::mk_device_from_fd, hub::Hub};
mod node_factory;
use node_factory::NodeType;
use serde::Serialize;
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::signal;
#[cfg(not(feature = "test_helpers"))]
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

    run_namespace_simulator(&args).await
}

async fn run_namespace_simulator(args: &SimArgs) -> Result<()> {
    let devices = Arc::new(Mutex::new(HashMap::new()));
    
    // Parse topology to get node names and count, and build delay matrix
    let topology_config = Config::builder()
        .add_source(config::File::with_name(&args.config_file))
        .build()?;
    
    let nodes = topology_config.get_table("nodes")?;
    let node_names: Vec<String> = nodes.keys().cloned().collect();
    let node_count = node_names.len();
    
    tracing::info!("Creating {} nodes: {:?}", node_count, node_names);
    
    // Create socketpairs for device communication - one pair per node
    let (node_fds, hub_fds) = mk_socketpairs(node_count)?;
    
    // Create MAC addresses for each node
    let node_macs: Vec<mac_address::MacAddress> = (0..node_count)
        .map(|i| {
            let mut bytes = [0u8; 6];
            bytes[0] = 0x02; // locally administered bit
            bytes[1] = (i as u8) + 1;
            bytes[2] = (i as u8) + 1;
            bytes[3] = (i as u8) + 1;
            bytes[4] = (i as u8) + 1;
            bytes[5] = (i as u8) + 1;
            bytes.into()
        })
        .collect();
    
    tracing::info!("Assigned MAC addresses: {:?}", node_macs);
    
    let node_names = Arc::new(node_names);
    let node_fds = Arc::new(node_fds);
    let node_macs = Arc::new(node_macs);
    
    let simulator = Simulator::new(&args, {
        let node_names = node_names.clone();
        let node_fds = node_fds.clone();
        let node_macs = node_macs.clone();
        let devices_clone = devices.clone();
        move |name, config| {
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
            let (tun_a, _peer) = test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };

        // Read optional cached_candidates; default to 3 when not present or invalid.
        let cached_candidates = settings
            .get_int("cached_candidates")
            .ok()
            .and_then(|x| u32::try_from(x).ok())
            .unwrap_or(3u32);

        let node_type = NodeType::from_str(&settings.get_string("node_type")?)?;
        let hello_history: u32 = settings.get_int("hello_history")?.try_into()?;
        let hello_periodicity = settings
            .get_int("hello_periodicity")
            .map(|x| u32::try_from(x).ok())
            .ok()
            .flatten();
        let enable_encryption = settings.get_bool("enable_encryption").unwrap_or(false);
        let ip = Some(Ipv4Addr::from_str(&settings.get_string("ip")?)?);

        #[cfg(not(feature = "test_helpers"))]
        let virtual_tun = Arc::new(Tun::new(
            TokioTun::builder()
                .tap()
                .name("virtual")
                .address(ip.unwrap())
                .mtu(1436)
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?,
        ));

        #[cfg(feature = "test_helpers")]
        let virtual_tun = {
            let (tun_a, _peer) = test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };

        // Find the index of this node to get the correct socketpair fd and MAC
        let node_index = node_names.iter().position(|n| n == name)
            .ok_or_else(|| anyhow::anyhow!("node {} not found in node list", name))?;
        let node_fd = node_fds[node_index];
        let node_mac = node_macs[node_index];
        
        tracing::info!("Creating node {} with index {}, MAC {}, fd {}", name, node_index, node_mac, node_fd);
        
        // Create device from socketpair fd instead of real network interface
        let dev = Arc::new(mk_device_from_fd(node_mac, node_fd));
        let node = node_factory::create_node_with_vdev(
            node_type,
            virtual_tun.name().to_string(),
            Some("virtual".to_string()),
            ip,
            1436,
            hello_history,
            hello_periodicity,
            cached_candidates,
            enable_encryption,
            virtual_tun.clone(),
            dev.clone(),
        )?;
        devices_clone
            .lock()
            .map_err(|e| anyhow::anyhow!("devices mutex poisoned: {}", e))?
            .insert(name.to_string(), (dev.clone(), virtual_tun.clone()));
        Ok((dev, virtual_tun, node))
        }
    })?;

    // Create Hub for device communication with latency matrix
    let topology = topology_config.get_table("topology")?;
    let mut delays_ms = vec![vec![0u64; node_count]; node_count];
    
    // Build delay matrix from topology configuration
    for (from_node, connections) in &topology {
        if let Some(from_index) = node_names.iter().position(|n| n == from_node) {
            let connections = connections.clone().into_table().unwrap_or_default();
            for (to_node, params) in connections {
                if let Some(to_index) = node_names.iter().position(|n| n == &to_node) {
                    let params = params.into_table().unwrap_or_default();
                    if let Ok(latency_ms) = params.get("latency").unwrap_or(&config::Value::from(0)).clone().into_int() {
                        delays_ms[from_index][to_index] = latency_ms as u64;
                        tracing::info!("Set delay from {} to {}: {}ms", from_node, to_node, latency_ms);
                    }
                }
            }
        }
    }
    
    // Create and start Hub with latency configuration
    let hub = Hub::new_with_mocked_time(hub_fds, delays_ms);
    hub.spawn();
    tracing::info!("Hub started for device communication with {} nodes", node_count);

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
                    let node_type = node.node_type_name().to_string();
                    let upstream_route = node.cached_upstream_route().and_then(|r| {
                        Some(UpstreamInfo {
                            hops: r.hops,
                            mac: format!("{}", r.mac),
                            node_name: mac_map.get(&r.mac).cloned(),
                        })
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
