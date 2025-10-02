#[cfg(feature = "webview")]
use crate::simulator::Channel;
use anyhow::{bail, Result};
use clap::{Parser, ValueEnum};
#[cfg(feature = "webview")]
use common::network_interface::NetworkInterface;
#[cfg(not(feature = "test_helpers"))]
use common::tun::Tun;
use config::Config;
#[cfg(feature = "webview")]
use itertools::Itertools;
use node_lib::args::NodeType;
#[cfg(test)]
use node_lib::PACKET_BUFFER_SIZE;
#[cfg(any(test, feature = "webview"))]
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::signal;
// Context is unused in current builds; remove the import.
#[cfg(not(feature = "test_helpers"))]
use tokio_tun::Tun as TokioTun;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
#[cfg(feature = "webview")]
use warp::Filter;

mod sim_args;
use sim_args::SimArgs;

mod node_factory;
mod simulator;
use node_factory::create_node_from_settings;
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

#[cfg(any(test, feature = "webview"))]
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
        tracing::debug!(?settings, "Node configuration loaded");

        #[cfg(not(feature = "test_helpers"))]
        let tun = Arc::new({
            let real_tun = TokioTun::builder()
                .name("real")
                .tap()
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?;
            Tun::new_real(real_tun)
        });
        #[cfg(feature = "test_helpers")]
        let tun = {
            // test build: use shared test helper to construct a shim Tun and take one end.
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Arc::new(tun_a)
        };

        // Parse node type from config. Map parsing errors into anyhow::Error so `?` works.
        let node = NodeType::from_str(&settings.get_string("node_type")?, true)
            .map_err(|e| anyhow::anyhow!(e))?;

        let (dev, _virtual_tun, node) = create_node_from_settings(node, &settings, tun.clone())?;
        devices
            .lock()
            .map_err(|e| anyhow::anyhow!("devices mutex poisoned: {}", e))?
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

                    use obu_lib::Obu;
                    use rsu_lib::Rsu;
                    let node_type = if node.as_any().is::<Obu>() {
                        "Obu".to_string()
                    } else if node.as_any().is::<Rsu>() {
                        "Rsu".to_string()
                    } else {
                        "Unknown".to_string()
                    };
                    let upstream_route = node
                        .as_any()
                        .downcast_ref::<Obu>()
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::channel_parameters::ChannelParameters;
    use mac_address::MacAddress;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn channel_set_params_updates_and_allows_send() {
        // Create a dummy tun from test helpers
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        // Channel::new spawns a background task; use a small topology-style from/to names
        let ch = crate::simulator::Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // Setting params via set_params should accept a valid map
        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "0".to_string());

        assert!(ch.set_params(map).is_ok());

        // Now exercise send/should_send by sending a packet with the correct MAC
        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        // destination mac = our mac
        packet[0..6].copy_from_slice(&mac.bytes());
        // payload small
        packet[6] = 0x42;

        let res = ch.send(packet, 7).await;
        assert!(res.is_ok());
    }

    #[test]
    fn error_message_serialize() {
        let em = ErrorMessage {
            code: 404,
            message: "not found".to_string(),
        };

        let v = serde_json::to_value(&em).expect("serialize");
        assert_eq!(v["code"].as_i64().unwrap(), 404);
        assert_eq!(v["message"].as_str().unwrap(), "not found");
    }
}
