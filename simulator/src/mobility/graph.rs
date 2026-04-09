//! Road network graph built from OSM data.
//!
//! Provides Dijkstra-based routing, coordinate projection (WGS84 → local
//! metres), and random node selection.

use crate::mobility::osm::{BoundingBox, OsmNode, OsmWay};
use anyhow::{bail, Result};
use petgraph::algo::dijkstra;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use rand::Rng;
use std::collections::HashMap;

/// Equirectangular projection constants.
const METRES_PER_LAT_DEG: f64 = 110_540.0;
const METRES_PER_LON_DEG_AT_EQUATOR: f64 = 111_320.0;

/// Project a WGS84 coordinate to local (x, y) metres relative to an origin.
///
/// `origin` is `(lat₀, lon₀)` — the bounding-box midpoint.
pub fn project(lat: f64, lon: f64, origin: (f64, f64)) -> (f64, f64) {
    let (lat0, lon0) = origin;
    let x = (lon - lon0) * lat0.to_radians().cos() * METRES_PER_LON_DEG_AT_EQUATOR;
    let y = (lat - lat0) * METRES_PER_LAT_DEG;
    (x, y)
}

/// Back-project local (x, y) metres to WGS84 lat/lon.
pub fn back_project(x: f64, y: f64, origin: (f64, f64)) -> (f64, f64) {
    let (lat0, lon0) = origin;
    let lat = y / METRES_PER_LAT_DEG + lat0;
    let lon = x / (lat0.to_radians().cos() * METRES_PER_LON_DEG_AT_EQUATOR) + lon0;
    (lat, lon)
}

/// Directed road graph built from OSM data.
pub struct RoadGraph {
    /// The underlying petgraph directed graph; edge weights are distances in metres.
    graph: DiGraph<OsmNode, f64>,
    /// Map from OSM node ID to graph NodeIndex (reserved for future ID-based lookup).
    #[allow(dead_code)]
    node_index: HashMap<i64, NodeIndex>,
    /// Projection origin (lat₀, lon₀) — bounding-box midpoint.
    pub origin: (f64, f64),
}

impl RoadGraph {
    /// Build the graph from parsed OSM data.
    ///
    /// `origin` is typically the bounding-box midpoint.
    /// Bidirectional ways get edges in both directions.
    pub fn from_osm(nodes: &[OsmNode], ways: &[OsmWay], origin: (f64, f64)) -> Self {
        let mut graph = DiGraph::new();
        let mut node_index: HashMap<i64, NodeIndex> = HashMap::new();

        // Add all nodes to the graph
        for osm_node in nodes {
            let idx = graph.add_node(osm_node.clone());
            node_index.insert(osm_node.id, idx);
        }

        // Add edges from ways
        for way in ways {
            for window in way.node_ids.windows(2) {
                let (a_id, b_id) = (window[0], window[1]);
                let (Some(&a_idx), Some(&b_idx)) = (node_index.get(&a_id), node_index.get(&b_id))
                else {
                    continue;
                };

                let a_node = &graph[a_idx];
                let b_node = &graph[b_idx];
                let (ax, ay) = project(a_node.lat, a_node.lon, origin);
                let (bx, by) = project(b_node.lat, b_node.lon, origin);
                let dist = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();

                // Forward direction always added
                graph.add_edge(a_idx, b_idx, dist);

                // Reverse direction for bidirectional roads
                if !way.oneway {
                    graph.add_edge(b_idx, a_idx, dist);
                }
            }
        }

        tracing::debug!(
            vertices = graph.node_count(),
            edges = graph.edge_count(),
            "Road graph built"
        );

        Self {
            graph,
            node_index,
            origin,
        }
    }

    /// Route from `from` to `to` using Dijkstra; returns ordered waypoints or `None`.
    pub fn route(&self, from: NodeIndex, to: NodeIndex) -> Option<Vec<NodeIndex>> {
        let costs = dijkstra(&self.graph, from, Some(to), |e| *e.weight());
        if !costs.contains_key(&to) {
            return None;
        }

        // Reconstruct path by greedy predecessor walk
        let mut path = vec![to];
        let mut current = to;
        while current != from {
            // Find neighbour with cost = current_cost - edge_weight that minimises cost
            let current_cost = costs[&current];
            let mut found = false;
            for edge in self
                .graph
                .edges_directed(current, petgraph::Direction::Incoming)
            {
                let pred = edge.source();
                let pred_cost = costs.get(&pred).copied().unwrap_or(f64::INFINITY);
                if (pred_cost + edge.weight() - current_cost).abs() < 1e-6 {
                    path.push(pred);
                    current = pred;
                    found = true;
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        path.reverse();
        Some(path)
    }

    /// Pick a random node from the graph.
    ///
    /// Only considers nodes that have at least one outgoing edge — isolated
    /// nodes (e.g. bbox-filtered bridge endpoints with all neighbours removed)
    /// are excluded so vehicles are never placed off-road.
    pub fn random_node(&self, rng: &mut impl Rng) -> NodeIndex {
        let routable: Vec<NodeIndex> = self
            .graph
            .node_indices()
            .filter(|&n| self.graph.edges(n).next().is_some())
            .collect();
        if routable.is_empty() {
            // Fallback: graph has no edges at all — pick any node.
            NodeIndex::new(rng.random_range(0..self.graph.node_count()))
        } else {
            routable[rng.random_range(0..routable.len())]
        }
    }

    /// Compute road-network distance in metres between two nodes using Dijkstra.
    pub fn distance_m(&self, a: NodeIndex, b: NodeIndex) -> f64 {
        let costs = dijkstra(&self.graph, a, Some(b), |e| *e.weight());
        costs.get(&b).copied().unwrap_or(f64::INFINITY)
    }

    /// Return the projected (x, y) in metres for a graph node.
    pub fn projected_pos(&self, idx: NodeIndex) -> (f64, f64) {
        let node = &self.graph[idx];
        project(node.lat, node.lon, self.origin)
    }

    /// Length of the directed edge from `a` to `b` in metres.
    pub fn edge_len(&self, a: NodeIndex, b: NodeIndex) -> f64 {
        self.graph
            .edges_connecting(a, b)
            .next()
            .map(|e| *e.weight())
            .unwrap_or_else(|| {
                // Fall back to Euclidean distance if edge missing
                let (ax, ay) = self.projected_pos(a);
                let (bx, by) = self.projected_pos(b);
                ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt()
            })
    }

    /// Find the graph node nearest to the given projected (x, y) position.
    pub fn nearest_node(&self, x: f64, y: f64) -> Result<NodeIndex> {
        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        if indices.is_empty() {
            bail!("Road graph is empty");
        }
        let nearest = indices
            .iter()
            .copied()
            .min_by(|&a, &b| {
                let (ax, ay) = self.projected_pos(a);
                let (bx, by) = self.projected_pos(b);
                let da = (ax - x).powi(2) + (ay - y).powi(2);
                let db = (bx - x).powi(2) + (by - y).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        Ok(nearest)
    }

    /// Total number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Compute the origin (midpoint) for a bounding box.
    pub fn origin_for_bbox(bbox: &BoundingBox) -> (f64, f64) {
        let lat0 = (bbox.min_lat + bbox.max_lat) / 2.0;
        let lon0 = (bbox.min_lon + bbox.max_lon) / 2.0;
        (lat0, lon0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobility::osm::{OsmNode, OsmWay};

    fn simple_graph() -> RoadGraph {
        let nodes = vec![
            OsmNode {
                id: 1,
                lat: 0.0,
                lon: 0.0,
            },
            OsmNode {
                id: 2,
                lat: 0.001,
                lon: 0.0,
            },
            OsmNode {
                id: 3,
                lat: 0.002,
                lon: 0.0,
            },
        ];
        let ways = vec![OsmWay {
            id: 1,
            node_ids: vec![1, 2, 3],
            oneway: false,
        }];
        RoadGraph::from_osm(&nodes, &ways, (0.001, 0.0))
    }

    #[test]
    fn route_finds_path() {
        let g = simple_graph();
        let start = NodeIndex::new(0);
        let end = NodeIndex::new(2);
        let path = g.route(start, end);
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.first().copied(), Some(start));
        assert_eq!(path.last().copied(), Some(end));
    }

    #[test]
    fn projection_roundtrip() {
        let origin = (41.155, -8.620);
        let lat = 41.160;
        let lon = -8.615;
        let (x, y) = project(lat, lon, origin);
        let (rlat, rlon) = back_project(x, y, origin);
        assert!((rlat - lat).abs() < 1e-9);
        assert!((rlon - lon).abs() < 1e-9);
    }

    #[test]
    fn distance_m_positive() {
        let g = simple_graph();
        let d = g.distance_m(NodeIndex::new(0), NodeIndex::new(2));
        assert!(d > 0.0);
        assert!(d.is_finite());
    }
}
