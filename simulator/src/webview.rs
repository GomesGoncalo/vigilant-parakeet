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
    let sim_nodes_full = simulator.get_nodes_with_interfaces();
    let channels = simulator.get_channels();
    let metrics = simulator.get_metrics();
    let rssi_tables = simulator.get_rssi_tables();
    let nakagami_config = simulator.get_nakagami_config().cloned();

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
                use serde_json::json;
                let map: HashMap<String, serde_json::Value> = sim_nodes
                    .iter()
                    .map(|(node, (device, tun, _node))| {
                        let dev = {
                            #[cfg(feature = "stats")]
                            {
                                serde_json::to_value(device.stats())
                                    .unwrap_or(serde_json::Value::Null)
                            }
                            #[cfg(not(feature = "stats"))]
                            {
                                serde_json::Value::Null
                            }
                        };
                        let tunv = {
                            #[cfg(feature = "stats")]
                            {
                                serde_json::to_value(tun.stats()).unwrap_or(serde_json::Value::Null)
                            }
                            #[cfg(not(feature = "stats"))]
                            {
                                serde_json::Value::Null
                            }
                        };
                        (node.clone(), json!({"device": dev, "tun": tunv}))
                    })
                    .collect();
                warp::reply::json(&map)
            })
    };

    // Endpoint: GET /node/<name> - returns device+tun stats for a specific node
    let node_stats_endpoint = {
        let sim_nodes = sim_nodes.clone();
        warp::get()
            .and(warp::path!("node" / String))
            .and(warp::path::end())
            .map(move |node: String| {
                let Some(reply) = sim_nodes.get(&node).map(|(device, tun, _node)| {
                    let dev = {
                        #[cfg(feature = "stats")]
                        {
                            serde_json::to_value(device.stats()).unwrap_or(serde_json::Value::Null)
                        }
                        #[cfg(not(feature = "stats"))]
                        {
                            serde_json::Value::Null
                        }
                    };
                    let tunv = {
                        #[cfg(feature = "stats")]
                        {
                            serde_json::to_value(tun.stats()).unwrap_or(serde_json::Value::Null)
                        }
                        #[cfg(not(feature = "stats"))]
                        {
                            serde_json::Value::Null
                        }
                    };
                    (dev, tunv)
                }) else {
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
        let sim_nodes_full = sim_nodes_full.clone();
        let rssi_tables = rssi_tables.clone();
        warp::get()
            .and(warp::path("node_info"))
            .and(warp::path::end())
            .map(move || {
                // Build a mapping of MAC -> node name
                let mac_map: HashMap<mac_address::MacAddress, String> = sim_nodes_full
                    .iter()
                    .map(|(name, (dev, _, _))| (dev.mac_address(), name.clone()))
                    .collect();

                #[derive(serde::Serialize, Clone)]
                struct UpstreamInfo {
                    hops: u32,
                    mac: String,
                    node_name: Option<String>,
                    rssi_dbm: Option<f32>,
                    latency_us: Option<u64>,
                }

                #[derive(serde::Serialize)]
                struct NodeInfo {
                    node_type: String,
                    mac: String,
                    cloud_ip: Option<String>,
                    virtual_ip: Option<String>,
                    has_session: bool,
                    upstream: Option<UpstreamInfo>,
                    downstream: Option<Vec<UpstreamInfo>>,
                }

                use crate::simulator::SimNode;
                use obu_lib::Obu;

                let mut out: HashMap<String, NodeInfo> = HashMap::new();
                let mut upstream_map: HashMap<String, UpstreamInfo> = HashMap::new();

                for (name, (dev, interfaces, node)) in sim_nodes_full.iter() {
                    let node_type = match node {
                        SimNode::Obu(_) => "Obu",
                        SimNode::Rsu(_) => "Rsu",
                        SimNode::Server(_) => "Server",
                    };
                    let obu = node.as_any().downcast_ref::<Obu>();
                    let has_session = obu.map(|o| o.has_dh_session()).unwrap_or(false);
                    let upstream_route = obu.and_then(|o| o.cached_upstream_route()).map(|r| {
                        let rssi_dbm = rssi_tables
                            .get(name)
                            .and_then(|tbl| tbl.read().ok())
                            .and_then(|guard| guard.get(&r.mac).copied());
                        UpstreamInfo {
                            hops: r.hops,
                            mac: format!("{}", r.mac),
                            node_name: mac_map.get(&r.mac).cloned(),
                            rssi_dbm,
                            latency_us: r.latency.map(|d| d.as_micros() as u64),
                        }
                    });

                    if let Some(ref u) = upstream_route {
                        upstream_map.insert(name.clone(), u.clone());
                    }

                    out.insert(
                        name.clone(),
                        NodeInfo {
                            node_type: node_type.to_string(),
                            mac: format!("{}", dev.mac_address()),
                            cloud_ip: interfaces.cloud_ip.map(|ip| ip.to_string()),
                            virtual_ip: interfaces.virtual_ip.map(|ip| ip.to_string()),
                            has_session,
                            upstream: upstream_route,
                            downstream: None,
                        },
                    );
                }

                // Invert upstream_map to produce downstream vectors per RSU/relay name.
                let mut downstream_map: HashMap<String, Vec<UpstreamInfo>> = HashMap::new();
                for (child_name, upinfo) in upstream_map.iter() {
                    if let Some(parent_name) = &upinfo.node_name {
                        let mut di = upinfo.clone();
                        di.node_name = Some(child_name.clone());
                        downstream_map
                            .entry(parent_name.clone())
                            .or_default()
                            .push(di);
                    }
                }
                for (node_name, nd) in downstream_map {
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

    // Endpoint: GET /memory — jemalloc allocator stats for live memory diagnostics.
    // Refresh the epoch first so stats reflect the current state.
    let memory_endpoint = warp::get()
        .and(warp::path("memory"))
        .and(warp::path::end())
        .map(|| {
            // Advance the jemalloc epoch so stats are up-to-date.
            let _ = jemalloc_ctl::epoch::mib().and_then(|m| m.advance());

            let allocated = jemalloc_ctl::stats::allocated::read().unwrap_or(0);
            let active = jemalloc_ctl::stats::active::read().unwrap_or(0);
            let resident = jemalloc_ctl::stats::resident::read().unwrap_or(0);
            let retained = jemalloc_ctl::stats::retained::read().unwrap_or(0);
            let mapped = jemalloc_ctl::stats::mapped::read().unwrap_or(0);

            warp::reply::json(&serde_json::json!({
                // Bytes of live allocations (your data structures).
                "allocated_mb":  allocated  as f64 / 1_048_576.0,
                // Bytes in active (live) pages — rounds up to page boundary.
                "active_mb":     active     as f64 / 1_048_576.0,
                // Bytes mapped into the process from the OS.
                "mapped_mb":     mapped     as f64 / 1_048_576.0,
                // Bytes in resident physical pages.
                "resident_mb":   resident   as f64 / 1_048_576.0,
                // Bytes retained by jemalloc but not yet returned to OS.
                "retained_mb":   retained   as f64 / 1_048_576.0,
                // Overhead = active − allocated (internal fragmentation).
                "frag_mb":       (active.saturating_sub(allocated)) as f64 / 1_048_576.0,
            }))
        });

    // Endpoint: GET /fading — returns fading config (max_range_m, etc.) for visualisation.
    let fading_endpoint = {
        warp::get()
            .and(warp::path("fading"))
            .and(warp::path::end())
            .map(move || {
                if let Some(ref cfg) = nakagami_config {
                    warp::reply::json(&serde_json::json!({
                        "enabled": true,
                        "max_range_m": cfg.max_range_m,
                        "m": cfg.m,
                        "eta": cfg.eta,
                        "snr_0_db": cfg.snr_0_db,
                        "snr_thresh_db": cfg.snr_thresh_db,
                        "latency_ms_per_100m": cfg.latency_ms_per_100m,
                        "update_ms": cfg.update_ms,
                    }))
                } else {
                    warp::reply::json(&serde_json::json!({ "enabled": false }))
                }
            })
    };

    let base_routes = nodes_endpoint
        .or(node_stats_endpoint)
        .or(stats_endpoint)
        .or(channels_get_endpoint)
        .or(node_info_endpoint)
        .or(channel_post_endpoint)
        .or(metrics_endpoint)
        .or(memory_endpoint);

    // Combine all endpoints
    let positions = simulator.get_positions();
    let override_queue = simulator.get_override_queue();

    // GET /positions — return all current node positions as JSON
    let positions_get_endpoint = {
        let positions = positions.clone();
        warp::get()
            .and(warp::path("positions"))
            .and(warp::path::end())
            .then(move || {
                let positions = positions.clone();
                async move {
                    let map = positions.read().await;
                    warp::reply::json(&*map)
                }
            })
    };

    // POST /node/<name>/position — override a node's position
    // Body: { "lat": f64, "lon": f64 }
    let position_post_endpoint = {
        warp::post()
            .and(warp::path!("node" / String / "position"))
            .and(warp::path::end())
            .and(warp::body::json())
            .and_then(move |name: String, body: PositionOverride| {
                let override_queue = override_queue.clone();
                async move { position_override_fn(name, body, override_queue).await }
            })
    };

    base_routes
        .or(fading_endpoint)
        .or(positions_get_endpoint)
        .or(position_post_endpoint)
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

/// Body for `POST /node/<name>/position`.
#[derive(serde::Deserialize)]
struct PositionOverride {
    lat: f64,
    lon: f64,
}

/// Handler for `POST /node/<name>/position`.
///
/// Enqueues a position override that the mobility tick loop will apply on its
/// next iteration.  For OBUs this triggers a route replan from the nearest OSM
/// node; for RSUs/Servers it simply updates the fixed position.
async fn position_override_fn(
    name: String,
    body: PositionOverride,
    override_queue: Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>>,
) -> Result<impl warp::Reply, warp::Rejection> {
    #[derive(serde::Serialize)]
    struct StatusResponse {
        status: &'static str,
    }

    override_queue
        .lock()
        .await
        .insert(name, (body.lat, body.lon));

    Ok(warp::reply::with_status(
        warp::reply::json(&StatusResponse { status: "queued" }),
        warp::http::StatusCode::ACCEPTED,
    ))
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
