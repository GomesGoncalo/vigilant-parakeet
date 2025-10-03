//! HTTP endpoints for monitoring and controlling the simulation via browser
//!
//! This module provides a warp-based HTTP server with endpoints for:
//! - Listing nodes and querying per-node statistics
//! - Viewing and modifying channel parameters (latency, loss, etc.)
//! - Retrieving routing topology information (upstream/downstream relationships)

use crate::channel::Channel;
use crate::simulator::Simulator;
use common::network_interface::NetworkInterface;
use itertools::Itertools;
use std::collections::HashMap;
use std::sync::Arc;
use warp::Filter;

/// Error response structure for HTTP 4xx/5xx responses
#[derive(serde::Serialize)]
pub struct ErrorMessage {
    pub code: u16,
    pub message: String,
}

/// Setup all webview HTTP endpoints and return a warp Filter
///
/// Endpoints provided:
/// - GET /nodes - List all node names
/// - GET /stats - Get stats for all nodes (device + tun stats)
/// - GET /node/<name> - Get stats for a specific node
/// - GET /channels - List all channel parameters
/// - POST /channel/<src>/<dst> - Update channel parameters
/// - GET /node_info - Get node type and routing topology info
pub fn setup_routes(
    simulator: &Simulator,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let sim_nodes = simulator.get_nodes();
    let channels = simulator.get_channels();
    let metrics = simulator.get_metrics();

    // Endpoint: GET /nodes - returns list of node names
    let nodes_endpoint = {
        let sim_nodes = sim_nodes.clone();
        warp::get()
            .and(warp::path("nodes"))
            .and(warp::path::end())
            .map(move || warp::reply::json(&sim_nodes.keys().cloned().collect_vec()))
    };

    // Endpoint: GET /stats - returns device+tun stats for all nodes
    let stats_endpoint = {
        let sim_nodes = sim_nodes.clone();
        warp::get()
            .and(warp::path("stats"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(
                    &sim_nodes
                        .iter()
                        .map(|(node, (device, tun, _node))| (node, (device.stats(), tun.stats())))
                        .collect::<HashMap<_, _>>(),
                )
            })
    };

    // Endpoint: GET /node/<name> - returns device+tun stats for a specific node
    let node_stats_endpoint = {
        let sim_nodes = sim_nodes.clone();
        warp::get()
            .and(warp::path!("node" / String))
            .and(warp::path::end())
            .map(move |node: String| {
                let Some(reply) = sim_nodes
                    .get(&node)
                    .map(|(device, tun, _node)| (device.stats(), tun.stats()))
                else {
                    let json = warp::reply::json(&ErrorMessage {
                        code: 404,
                        message: "node not found".to_string(),
                    });
                    return warp::reply::with_status(json, warp::http::StatusCode::NOT_FOUND);
                };

                let mut map = HashMap::with_capacity(1);
                map.insert(node, reply);
                let json = warp::reply::json(&map);
                warp::reply::with_status(json, warp::http::StatusCode::OK)
            })
    };

    // Endpoint: GET /channels - returns channel parameters for all node pairs
    let channels_get_endpoint = {
        let channels = channels.clone();
        warp::get()
            .and(warp::path("channels"))
            .and(warp::path::end())
            .map(move || {
                warp::reply::json(
                    &channels
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
            })
    };

    // Endpoint: POST /channel/<src>/<dst> - update channel parameters
    let channel_post_endpoint = {
        let channels = Arc::new(channels.clone());
        warp::post()
            .and(warp::path!("channel" / String / String))
            .and(warp::path::end())
            .and(warp::body::json())
            .and_then(move |src, dst, post| channel_post_fn(src, dst, post, channels.clone()))
    };

    // Endpoint: GET /node_info - returns node type and routing topology (upstream/downstream)
    let node_info_endpoint = {
        let sim_nodes = sim_nodes.clone();
        warp::get()
            .and(warp::path("node_info"))
            .and(warp::path::end())
            .map(move || {
                // Build a mapping of MAC -> node name
                let mac_map: HashMap<mac_address::MacAddress, String> = sim_nodes
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

                    use crate::simulator::SimNode;
                    use obu_lib::Obu;
                    let node_type = match node {
                        SimNode::Obu(_) => "Obu".to_string(),
                        SimNode::Rsu(_) => "Rsu".to_string(),
                        SimNode::Server(_) => "Server".to_string(),
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
            })
    };

    // Endpoint: GET /metrics - returns simulation metrics
    let metrics_endpoint = {
        let metrics = metrics.clone();
        warp::get()
            .and(warp::path("metrics"))
            .and(warp::path::end())
            .map(move || {
                let summary = metrics.summary();
                warp::reply::json(&serde_json::json!({
                    "packets_sent": summary.packets_sent,
                    "packets_dropped": summary.packets_dropped,
                    "packets_delayed": summary.packets_delayed,
                    "total_packets": summary.total_packets,
                    "drop_rate": summary.drop_rate,
                    "avg_latency_ms": summary.avg_latency_ms(),
                    "active_channels": summary.active_channels,
                    "active_nodes": summary.active_nodes,
                    "uptime_secs": summary.uptime.as_secs_f64(),
                    "throughput_pps": summary.packets_per_second(),
                }))
            })
    };

    // CORS configuration to allow requests from visualization frontend
    let cors = warp::cors()
        .allow_any_origin()
        .allow_methods(vec!["GET", "POST", "OPTIONS"])
        .allow_headers(vec!["content-type"]);

    // Combine all endpoints
    nodes_endpoint
        .or(node_stats_endpoint)
        .or(stats_endpoint)
        .or(channels_get_endpoint)
        .or(node_info_endpoint)
        .or(channel_post_endpoint)
        .or(metrics_endpoint)
        .with(cors)
}

/// Handler for POST /channel/<src>/<dst> - update channel parameters
async fn channel_post_fn(
    src: String,
    dst: String,
    post: HashMap<String, String>,
    channels: Arc<HashMap<String, HashMap<String, Arc<Channel>>>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    // Validate that the channel exists
    let Some(onode) = channels.get(&src) else {
        return Ok(warp::reply::with_status(
            warp::reply::json(&ErrorMessage {
                code: 404,
                message: format!("source node {} not found", src),
            }),
            warp::http::StatusCode::NOT_FOUND,
        ));
    };

    let Some(channel) = onode.get(&dst) else {
        return Ok(warp::reply::with_status(
            warp::reply::json(&ErrorMessage {
                code: 404,
                message: format!("channel {} -> {} not found", src, dst),
            }),
            warp::http::StatusCode::NOT_FOUND,
        ));
    };

    // Apply the new parameters
    #[derive(serde::Serialize)]
    struct StatusResponse {
        status: &'static str,
    }

    match channel.set_params(post) {
        Ok(()) => Ok(warp::reply::with_status(
            warp::reply::json(&StatusResponse { status: "ok" }),
            warp::http::StatusCode::OK,
        )),
        Err(e) => Ok(warp::reply::with_status(
            warp::reply::json(&ErrorMessage {
                code: 400,
                message: format!("failed to set channel params: {}", e),
            }),
            warp::http::StatusCode::BAD_REQUEST,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
