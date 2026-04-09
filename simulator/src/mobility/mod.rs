//! Mobility manager — orchestrates the OSM road network, vehicle states, and
//! the asynchronous physics tick loop.
//!
//! # Overview
//!
//! On startup the manager:
//! 1. Fetches (or loads from cache) the road network for the configured bounding box.
//! 2. Builds a directed `RoadGraph` from the OSM data.
//! 3. For each OBU in the simulation it places the vehicle at a random road node
//!    and plans a route to a destination at least `min_trip_distance_m` away.
//! 4. For each RSU/Server it records a fixed projected position.
//!
//! The `run_loop` method should be `tokio::spawn`-ed; it fires a tick every
//! `tick_ms` milliseconds and writes updated `NodePosition` values into the
//! shared `Arc<RwLock<…>>` so the HTTP layer can serve them.

pub mod graph;
pub mod osm;
pub mod position;
pub mod vehicle;

use crate::mobility::{
    graph::{back_project, project, RoadGraph},
    osm::{fetch_osm, BoundingBox},
    position::NodePosition,
    vehicle::VehicleState,
};
use anyhow::{bail, Result};
use petgraph::graph::NodeIndex;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

/// Per-edge occupancy: edge `(from, to)` → list of `(vehicle_name, progress_m, speed_ms)`.
type EdgeOccupancy = HashMap<(NodeIndex, NodeIndex), Vec<(String, f64, f64)>>;

/// Configuration parsed from the `mobility:` block in `simulator.yaml`.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct MobilityConfig {
    /// Enable the mobility system.
    #[serde(default)]
    pub enabled: bool,
    /// WGS84 bounding box of the city area.
    pub bbox: Option<BoundingBox>,
    /// Path to the OSM JSON cache file.
    #[serde(default = "default_cache_path")]
    pub osm_cache: String,
    /// Physics update interval in milliseconds.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
    /// Minimum trip distance in metres.
    #[serde(default = "default_min_trip_distance_m")]
    pub min_trip_distance_m: f64,
    /// Default desired speed for vehicles in m/s.
    #[serde(default = "default_desired_speed_ms")]
    pub desired_speed_ms: f64,
}

fn default_cache_path() -> String {
    "osm_cache.json".to_string()
}
fn default_tick_ms() -> u64 {
    100
}
fn default_min_trip_distance_m() -> f64 {
    804.0
}
fn default_desired_speed_ms() -> f64 {
    13.9
}

/// Per-node geographic configuration (optional in node YAML files).
#[derive(Debug, Clone, Default)]
pub struct NodeGeoConfig {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// Mobility manager: owns the road graph, vehicle states and fixed positions.
pub struct MobilityManager {
    graph: Arc<RoadGraph>,
    /// OBU name → vehicle state.
    vehicles: HashMap<String, VehicleState>,
    /// RSU/Server name → fixed projected (x, y).
    fixed: HashMap<String, (f64, f64)>,
    /// Shared positions map updated every tick; exposed via HTTP.
    positions: Arc<RwLock<HashMap<String, NodePosition>>>,
    /// Pending position overrides from the HTTP layer: name → (lat, lon).
    /// The tick loop drains this on each tick and replans affected vehicles.
    override_queue: Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>>,
    min_trip_distance_m: f64,
    tick_duration: Duration,
    desired_speed_ms: f64,
    /// (lat₀, lon₀) bounding-box midpoint for back-projection.
    origin: (f64, f64),
}

impl MobilityManager {
    /// Construct and initialise the mobility manager.
    ///
    /// `node_configs` maps node names to their type ("Obu", "Rsu", "Server")
    /// and optional geographic coordinates.
    pub async fn new(
        config: MobilityConfig,
        node_configs: HashMap<String, (String, NodeGeoConfig)>,
    ) -> Result<Self> {
        let bbox = config
            .bbox
            .ok_or_else(|| anyhow::anyhow!("mobility.bbox is required when mobility is enabled"))?;
        let origin = RoadGraph::origin_for_bbox(&bbox);

        let (mut osm_nodes, osm_ways) = fetch_osm(bbox, &config.osm_cache).await?;

        // Drop nodes outside the bounding box. The Overpass query uses (._;>) which
        // pulls in every node referenced by a matching way, including bridge endpoints
        // that extend outside the bbox (e.g. into the Douro river). Filtering here
        // keeps vehicles on roads that are visually within the area of interest.
        osm_nodes.retain(|n| {
            n.lat >= bbox.min_lat
                && n.lat <= bbox.max_lat
                && n.lon >= bbox.min_lon
                && n.lon <= bbox.max_lon
        });

        if osm_nodes.is_empty() {
            bail!("No OSM nodes found for the given bounding box — check connectivity or bbox");
        }

        let graph = Arc::new(RoadGraph::from_osm(&osm_nodes, &osm_ways, origin));
        tracing::info!(
            nodes = graph.node_count(),
            origin_lat = origin.0,
            origin_lon = origin.1,
            "Road graph ready"
        );

        let mut rng = SmallRng::from_rng(&mut rand::rng());
        let positions = Arc::new(RwLock::new(HashMap::new()));
        let mut vehicles: HashMap<String, VehicleState> = HashMap::new();
        let mut fixed: HashMap<String, (f64, f64)> = HashMap::new();

        for (name, (node_type, geo)) in &node_configs {
            match node_type.to_lowercase().as_str() {
                "obu" => {
                    let start = if let (Some(lat), Some(lon)) = (geo.lat, geo.lon) {
                        let (x, y) = project(lat, lon, origin);
                        graph.nearest_node(x, y)?
                    } else {
                        graph.random_node(&mut rng)
                    };

                    let dest =
                        sample_far_destination(&graph, start, config.min_trip_distance_m, &mut rng);
                    let route = graph.route(start, dest).unwrap_or_else(|| vec![start]);
                    let vehicle = VehicleState::new(
                        if route.len() >= 2 {
                            route
                        } else {
                            vec![start, start]
                        },
                        config.desired_speed_ms,
                        &graph,
                    );
                    let (x, y) = graph.projected_pos(start);
                    let (lat, lon) = back_project(x, y, origin);
                    positions.write().await.insert(
                        name.clone(),
                        NodePosition {
                            lat,
                            lon,
                            speed: 0.0,
                            bearing: 0.0,
                        },
                    );
                    vehicles.insert(name.clone(), vehicle);
                }
                _ => {
                    // RSU, Server — fixed position
                    let (x, y) = if let (Some(lat), Some(lon)) = (geo.lat, geo.lon) {
                        project(lat, lon, origin)
                    } else {
                        let rand_node = graph.random_node(&mut rng);
                        graph.projected_pos(rand_node)
                    };
                    let (lat, lon) = back_project(x, y, origin);
                    positions.write().await.insert(
                        name.clone(),
                        NodePosition {
                            lat,
                            lon,
                            speed: 0.0,
                            bearing: 0.0,
                        },
                    );
                    fixed.insert(name.clone(), (x, y));
                }
            }
        }

        Ok(Self {
            graph,
            vehicles,
            fixed,
            positions,
            override_queue: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            min_trip_distance_m: config.min_trip_distance_m,
            tick_duration: Duration::from_millis(config.tick_ms),
            desired_speed_ms: config.desired_speed_ms,
            origin,
        })
    }

    /// Shared positions map — pass this Arc to the HTTP layer.
    pub fn get_positions(&self) -> Arc<RwLock<HashMap<String, NodePosition>>> {
        self.positions.clone()
    }

    /// Override queue — pass this Arc to the HTTP layer for POST /node/<name>/position.
    pub fn get_override_queue(&self) -> Arc<tokio::sync::Mutex<HashMap<String, (f64, f64)>>> {
        self.override_queue.clone()
    }

    /// Async tick loop — `tokio::spawn` this after construction.
    pub async fn run_loop(mut self) {
        let mut interval = tokio::time::interval(self.tick_duration);
        let dt = self.tick_duration.as_secs_f64();
        loop {
            interval.tick().await;
            self.tick(dt).await;
        }
    }

    /// Advance all vehicles by `dt` seconds.
    async fn tick(&mut self, dt: f64) {
        let mut rng = SmallRng::from_rng(&mut rand::rng());
        let overrides: HashMap<String, (f64, f64)> = {
            let mut queue = self.override_queue.lock().await;
            std::mem::take(&mut *queue)
        };
        for (name, (lat, lon)) in overrides {
            let (x, y) = project(lat, lon, self.origin);
            if self.fixed.contains_key(&name) {
                self.fixed.insert(name.clone(), (x, y));
            } else if let Ok(nearest) = self.graph.nearest_node(x, y) {
                let dest = sample_far_destination(
                    &self.graph,
                    nearest,
                    self.min_trip_distance_m,
                    &mut rng,
                );
                if let Some(route) = self.graph.route(nearest, dest) {
                    if route.len() >= 2 {
                        self.vehicles.insert(
                            name.clone(),
                            VehicleState::new(route, self.desired_speed_ms, &self.graph),
                        );
                    }
                }
            }
        }

        // Build a per-edge occupancy map: edge (A, B) → vec of (name, progress_m, speed)
        let edge_occupancy: EdgeOccupancy = {
            let mut map: EdgeOccupancy = HashMap::new();
            for (name, v) in &self.vehicles {
                if v.route_idx + 1 < v.route.len() {
                    let edge = (v.route[v.route_idx], v.route[v.route_idx + 1]);
                    map.entry(edge)
                        .or_default()
                        .push((name.clone(), v.progress_m, v.speed));
                }
            }
            map
        };

        let graph = self.graph.clone();
        let min_trip = self.min_trip_distance_m;
        let desired_speed = self.desired_speed_ms;

        // Collect names first to avoid borrow issues
        let names: Vec<String> = self.vehicles.keys().cloned().collect();

        for name in &names {
            let v = self.vehicles.get_mut(name).unwrap();

            let (gap, leader_speed) = if v.route_idx + 1 < v.route.len() {
                let edge = (v.route[v.route_idx], v.route[v.route_idx + 1]);
                find_leader(
                    name,
                    v.progress_m,
                    &edge_occupancy.get(&edge).cloned().unwrap_or_default(),
                )
            } else {
                (f64::INFINITY, desired_speed)
            };

            let accel = v.idm_accel(gap, leader_speed);
            v.step(accel, dt, &graph);

            if v.is_done() {
                let current = v.current_node();
                let dest = sample_far_destination(&graph, current, min_trip, &mut rng);
                if let Some(route) = graph.route(current, dest) {
                    if route.len() >= 2 {
                        *v = VehicleState::new(route, desired_speed, &graph);
                    }
                }
            }
        }

        // Write updated positions
        let mut pos_map = self.positions.write().await;
        for (name, v) in &self.vehicles {
            let (lat, lon) = back_project(v.x, v.y, self.origin);
            pos_map.insert(
                name.clone(),
                NodePosition {
                    lat,
                    lon,
                    speed: v.speed,
                    bearing: v.bearing(),
                },
            );
        }
        for (name, (x, y)) in &self.fixed {
            let (lat, lon) = back_project(*x, *y, self.origin);
            pos_map.entry(name.clone()).or_insert(NodePosition {
                lat,
                lon,
                speed: 0.0,
                bearing: 0.0,
            });
        }
    }
}

/// Find the leader for a vehicle on an edge.
///
/// Returns `(gap_m, leader_speed)`. The leader is the vehicle with the highest
/// `progress_m` that is still strictly ahead of `my_progress`.
fn find_leader(my_name: &str, my_progress: f64, occupants: &[(String, f64, f64)]) -> (f64, f64) {
    let mut best_gap = f64::INFINITY;
    let mut best_speed = f64::INFINITY;

    for (name, progress, speed) in occupants {
        if name == my_name {
            continue;
        }
        if *progress > my_progress {
            let gap = progress - my_progress - 4.5; // subtract vehicle length
            if gap < best_gap {
                best_gap = gap.max(0.1);
                best_speed = *speed;
            }
        }
    }

    (best_gap, best_speed)
}

/// Sample a destination node that is at least `min_distance_m` away by road.
///
/// Makes up to 50 attempts; falls back to the farthest sampled node.
fn sample_far_destination(
    graph: &RoadGraph,
    start: NodeIndex,
    min_distance_m: f64,
    rng: &mut impl Rng,
) -> NodeIndex {
    let mut best_node = start;
    let mut best_dist = 0.0_f64;

    for _ in 0..50 {
        let candidate = graph.random_node(rng);
        if candidate == start {
            continue;
        }
        let d = graph.distance_m(start, candidate);
        if d >= min_distance_m {
            return candidate;
        }
        if d > best_dist {
            best_dist = d;
            best_node = candidate;
        }
    }

    best_node
}
