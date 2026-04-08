//! Vehicle kinematics and Intelligent Driver Model (IDM).

use crate::mobility::graph::RoadGraph;
use petgraph::graph::NodeIndex;

/// Per-vehicle kinematic state and IDM parameters.
pub struct VehicleState {
    /// Planned route as ordered graph node waypoints.
    pub route: Vec<NodeIndex>,
    /// Index into `route` of the *current* segment's start node.
    pub route_idx: usize,
    /// Metres travelled along the current edge.
    pub progress_m: f64,
    /// Length of the current edge in metres.
    pub edge_len_m: f64,

    /// Current speed in m/s.
    pub speed: f64,
    /// Projected x position in metres.
    pub x: f64,
    /// Projected y position in metres.
    pub y: f64,
    /// Previous x (used for bearing calculation).
    pub prev_x: f64,
    /// Previous y (used for bearing calculation).
    pub prev_y: f64,

    // IDM parameters
    /// Desired free-flow speed v₀ (m/s).
    pub desired_speed: f64,
    /// Maximum acceleration a (m/s²).
    pub max_accel: f64,
    /// Comfortable deceleration b (m/s²).
    pub comfortable_decel: f64,
    /// Minimum gap s₀ (m).
    pub min_gap_m: f64,
    /// Safe time headway T (s).
    pub time_headway_s: f64,
    /// Acceleration exponent δ.
    pub accel_exp: f64,
}

impl VehicleState {
    /// Construct a new vehicle on the given pre-computed route.
    pub fn new(route: Vec<NodeIndex>, desired_speed: f64, graph: &RoadGraph) -> Self {
        assert!(route.len() >= 2, "route must have at least 2 nodes");
        let edge_len_m = graph.edge_len(route[0], route[1]);
        let (x, y) = graph.projected_pos(route[0]);
        Self {
            route,
            route_idx: 0,
            progress_m: 0.0,
            edge_len_m,
            speed: 0.0,
            x,
            y,
            prev_x: x,
            prev_y: y,
            desired_speed,
            max_accel: 1.5,
            comfortable_decel: 2.0,
            min_gap_m: 2.0,
            time_headway_s: 1.5,
            accel_exp: 4.0,
        }
    }

    /// IDM acceleration in m/s² given gap to the leader ahead and leader speed.
    ///
    /// Pass `gap_m = f64::INFINITY` and `leader_speed = self.desired_speed` when
    /// there is no leader on the same edge.
    pub fn idm_accel(&self, gap_m: f64, leader_speed: f64) -> f64 {
        let dv = self.speed - leader_speed;
        let s_star = self.min_gap_m
            + f64::max(
                0.0,
                self.speed * self.time_headway_s
                    + self.speed * dv / (2.0 * (self.max_accel * self.comfortable_decel).sqrt()),
            );
        let gap = gap_m.max(0.1); // avoid division by zero
        self.max_accel
            * (1.0
                - (self.speed / self.desired_speed).powf(self.accel_exp)
                - (s_star / gap).powi(2))
    }

    /// Advance the vehicle by `dt` seconds given `accel` (m/s²).
    ///
    /// Updates speed, progress along the current edge, and (x, y) position.
    /// When the vehicle overshoots an edge it advances to the next waypoint.
    pub fn step(&mut self, accel: f64, dt: f64, graph: &RoadGraph) {
        self.prev_x = self.x;
        self.prev_y = self.y;

        // Euler integration – clamp speed ≥ 0
        self.speed = (self.speed + accel * dt).max(0.0);
        let ds = self.speed * dt;
        self.progress_m += ds;

        // Advance through waypoints while we overshoot edges
        while self.progress_m >= self.edge_len_m {
            self.progress_m -= self.edge_len_m;
            if self.route_idx + 2 < self.route.len() {
                self.route_idx += 1;
                let a = self.route[self.route_idx];
                let b = self.route[self.route_idx + 1];
                self.edge_len_m = graph.edge_len(a, b);
            } else {
                // Reached destination
                self.route_idx = self.route.len().saturating_sub(2);
                self.progress_m = 0.0;
                self.speed = 0.0;
                break;
            }
        }

        // Interpolate (x, y) along current edge
        let a_idx = self.route[self.route_idx];
        let (ax, ay) = graph.projected_pos(a_idx);
        let (bx, by) = if self.route_idx + 1 < self.route.len() {
            graph.projected_pos(self.route[self.route_idx + 1])
        } else {
            (ax, ay)
        };
        let t = if self.edge_len_m > 0.0 {
            (self.progress_m / self.edge_len_m).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.x = ax + (bx - ax) * t;
        self.y = ay + (by - ay) * t;
    }

    /// Returns `true` when the vehicle has reached the last waypoint.
    pub fn is_done(&self) -> bool {
        self.route_idx + 1 >= self.route.len() && self.speed == 0.0
    }

    /// Current start node of the edge being traversed.
    pub fn current_node(&self) -> NodeIndex {
        self.route[self.route_idx]
    }

    /// Bearing in degrees clockwise from north, computed from movement direction.
    pub fn bearing(&self) -> f64 {
        let dx = self.x - self.prev_x;
        let dy = self.y - self.prev_y;
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return 0.0;
        }
        // atan2(dx, dy): dx = east component, dy = north component → bearing from north
        let rad = dx.atan2(dy);
        let deg = rad.to_degrees();
        if deg < 0.0 {
            deg + 360.0
        } else {
            deg
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobility::osm::{OsmNode, OsmWay};

    fn make_graph() -> RoadGraph {
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
    fn idm_free_road_positive_accel() {
        let graph = make_graph();
        let route = vec![NodeIndex::new(0), NodeIndex::new(1), NodeIndex::new(2)];
        let v = VehicleState::new(route, 13.9, &graph);
        let a = v.idm_accel(f64::INFINITY, 13.9);
        // At speed 0, free road → positive acceleration
        assert!(a > 0.0);
    }

    #[test]
    fn step_advances_position() {
        let graph = make_graph();
        let route = vec![NodeIndex::new(0), NodeIndex::new(1), NodeIndex::new(2)];
        let mut v = VehicleState::new(route, 13.9, &graph);
        let (x0, y0) = (v.x, v.y);
        // Run several steps
        for _ in 0..20 {
            let a = v.idm_accel(f64::INFINITY, 13.9);
            v.step(a, 0.1, &graph);
        }
        let moved = (v.x - x0).abs() + (v.y - y0).abs();
        assert!(moved > 0.0, "vehicle should have moved");
    }

    #[test]
    fn vehicle_done_at_end() {
        let graph = make_graph();
        let route = vec![NodeIndex::new(0), NodeIndex::new(1)];
        let mut v = VehicleState::new(route, 13.9, &graph);
        // Force it to the end
        for _ in 0..5000 {
            if v.is_done() {
                break;
            }
            let a = v.idm_accel(f64::INFINITY, 13.9);
            v.step(a, 0.1, &graph);
        }
        assert!(v.is_done());
    }
}
