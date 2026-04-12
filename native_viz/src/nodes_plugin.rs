use crate::api::Snapshot;
use egui::{pos2, Color32, FontId, Pos2, Stroke};
use walkers::{lon_lat, MapMemory, Plugin, Position, Projector};

const OBU_COLOR: Color32 = Color32::from_rgb(30, 100, 220);
const RSU_COLOR: Color32 = Color32::from_rgb(220, 120, 30);
//const SERVER_COLOR: Color32 = Color32::from_rgb(140, 140, 140);
/// Longest upstream line (screen pixels) that maps to full red (fallback when RSSI unavailable).
const UPSTREAM_MAX_PX: f32 = 400.0;
/// Fallback RSU coverage radius (metres) used when the simulator has not yet
/// reported `max_range_m` via the `/fading` endpoint.
const RSU_RANGE_M_DEFAULT: f32 = 500.0;
const LABEL_BG: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 160);

/// walkers [`Plugin`] that draws every node in the current [`Snapshot`].
pub struct NodesPlugin {
    snapshot: Snapshot,
    prev_positions: std::collections::HashMap<String, crate::api::NodePosition>,
    prev_time: Option<std::time::Instant>,
    show_rsu_range: bool,
    highlighted_node: Option<String>,
    rsu_range_m: f32,
}

impl NodesPlugin {
    pub fn new(
        snapshot: &Snapshot,
        show_rsu_range: bool,
        highlighted_node: Option<String>,
    ) -> Self {
        let rsu_range_m = snapshot
            .max_range_m
            .map(|v| v as f32)
            .unwrap_or(RSU_RANGE_M_DEFAULT);
        Self {
            snapshot: snapshot.clone(),
            show_rsu_range,
            highlighted_node,
            rsu_range_m,
            prev_positions: snapshot.positions.clone(),
            prev_time: snapshot.last_positions_at,
        }
    }
}

impl Plugin for NodesPlugin {
    fn run(
        mut self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();

        // Scale reference radii from metres to screen pixels.
        let ref_pos = lon_lat(-8.625, 41.157);
        let px_per_m = projector.scale_pixel_per_meter(ref_pos);
        let obu_r = (8.0 * px_per_m).clamp(3.0, 14.0);
        let rsu_half = (10.0 * px_per_m).clamp(4.0, 18.0);

        let to_screen = |lat: f64, lon: f64| -> Pos2 {
            projector.project(lon_lat(lon, lat) as Position).to_pos2()
        };

        // --- Pass 0: RSU coverage range circles (radial gradient, deepest layer) ---
        if self.show_rsu_range {
            let range_px = self.rsu_range_m * px_per_m;
            for (name, pos) in &self.snapshot.positions {
                let is_rsu = self
                    .snapshot
                    .node_info
                    .get(name)
                    .map(|n| n.node_type == "Rsu")
                    .unwrap_or(false);
                if !is_rsu {
                    continue;
                }
                let center = to_screen(pos.lat, pos.lon);
                draw_rsu_range_circle(painter, center, range_px);
            }
        }

        // --- Pass 1: upstream routing lines (rendered below node symbols) ---
        for (name, info) in &self.snapshot.node_info {
            if info.node_type == "Server" {
                continue;
            }
            let Some(ref up) = info.upstream else {
                continue;
            };
            let Some(ref upstream_name) = up.node_name else {
                continue;
            };
            let Some(obu_pos) = self.snapshot.positions.get(name) else {
                continue;
            };
            let Some(rsu_pos) = self.snapshot.positions.get(upstream_name) else {
                continue;
            };
            let a = to_screen(obu_pos.lat, obu_pos.lon);
            let b = to_screen(rsu_pos.lat, rsu_pos.lon);
            // t = 0.0 → green (strong signal / short link)
            // t = 1.0 → red  (weak signal / long link)
            //
            // RSSI is in dBm (already log-scale).  Mapping dBm linearly to t
            // compresses the near/green end; instead we invert the path-loss
            // formula to get metres, then map that linearly to [0, 1].  This
            // way the colour matches what you see against the RSU range circle.
            let t = if let Some(rssi) = up.rssi_dbm {
                // Invert RSSI ≈ −20 − 20·log₁₀(d)  →  d = 10^((-rssi − 20) / 20)
                let dist_m = 10.0_f32.powf((-rssi - 20.0) / 20.0);
                // Apply gamma > 1 so the green end stretches further:
                // linear t would show green only for d < ~80 m; gamma=0.5 (sqrt)
                // makes the gradient perceptually even across the whole range.
                (dist_m / self.rsu_range_m).clamp(0.0, 1.0).powf(0.5)
            } else {
                ((a - b).length() / UPSTREAM_MAX_PX).clamp(0.0, 1.0)
            };
            let line_color = Color32::from_rgba_premultiplied(
                (t * 200.0) as u8,
                ((1.0 - t) * 180.0) as u8,
                0,
                180,
            );
            painter.line_segment([a, b], Stroke::new(5.0, line_color));
        }

        // --- Pass 2: node symbols ---
        let now = self.snapshot.last_positions_at.unwrap_or_else(std::time::Instant::now);
        let prev_time = self.prev_time.unwrap_or(now);
        let dt = (now - prev_time).as_secs_f32().clamp(0.0, 0.5);
        let interp = if dt > 0.0 && dt < 0.5 { (dt / 0.2).clamp(0.0, 1.0) } else { 1.0 };

        for (name, pos) in &self.snapshot.positions {
            let node_type = self
                .snapshot
                .node_info
                .get(name)
                .map(|n| n.node_type.as_str())
                .unwrap_or("Obu");

            if node_type == "Server" {
                continue;
            }

            let mut lat = pos.lat;
            let mut lon = pos.lon;
            let mut bearing = pos.bearing;
            let mut speed = pos.speed;

            if node_type == "Obu" {
                if let Some(prev) = self.prev_positions.get(name) {
                    // Linear interpolation for smoother animation
                    let interp = interp as f64;
                    lat = prev.lat + (pos.lat - prev.lat) * interp;
                    lon = prev.lon + (pos.lon - prev.lon) * interp;
                    bearing = prev.bearing + (pos.bearing - prev.bearing) * interp;
                    speed = prev.speed + (pos.speed - prev.speed) * interp;
                }
            }

            let screen = to_screen(lat, lon);

            match node_type {
                "Rsu" => {
                    draw_triangle(painter, screen, rsu_half, RSU_COLOR);
                }
                "Obu" => {
                    // OBU — filled circle with a thin white border.
                    painter.circle_filled(screen, obu_r, OBU_COLOR);
                    painter.circle_stroke(screen, obu_r, Stroke::new(1.0, Color32::WHITE));

                    // Bearing arrow when the vehicle is moving.
                    if speed > 0.5 {
                        let angle = bearing.to_radians() as f32;
                        let tip = pos2(
                            screen.x + angle.sin() * (obu_r + 6.0),
                            screen.y - angle.cos() * (obu_r + 6.0),
                        );
                        painter.line_segment([screen, tip], Stroke::new(4.0, Color32::WHITE));
                    }
                }
                _ => {}
            }

            // Highlight ring for the searched node.
            if self.highlighted_node.as_deref() == Some(name.as_str()) {
                let ring_r = if node_type == "Rsu" {
                    rsu_half * 1.8
                } else {
                    obu_r + 6.0
                };
                painter.circle_stroke(screen, ring_r, egui::Stroke::new(3.0, egui::Color32::WHITE));
                painter.circle_stroke(
                    screen,
                    ring_r + 3.5,
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 220, 0)),
                );
            }

            // Labels only when zoomed in enough.
            if obu_r > 6.0 {
                draw_label(painter, screen, name, obu_r);
            }
        }

        // Update previous positions and time for next frame
        self.prev_positions = self.snapshot.positions.clone();
        self.prev_time = self.snapshot.last_positions_at;
    }
}

/// Radial-gradient circle representing RSU coverage.
///
/// The mesh is a fan of triangles from the centre (opaque RSU orange) to the
/// outer ring (fully transparent), giving a smooth radial fade.
fn draw_rsu_range_circle(painter: &egui::Painter, center: Pos2, radius_px: f32) {
    const SEGMENTS: u32 = 64;
    // RSU orange, semi-opaque at the centre → transparent at the edge.
    let center_color = Color32::from_rgba_premultiplied(180, 90, 15, 55);
    let edge_color = Color32::TRANSPARENT;

    let mut mesh = egui::Mesh::default();
    // Vertex 0: centre
    mesh.colored_vertex(center, center_color);
    // Vertices 1..=SEGMENTS: outer ring
    for i in 0..SEGMENTS {
        let angle = i as f32 * std::f32::consts::TAU / SEGMENTS as f32;
        mesh.colored_vertex(
            pos2(
                center.x + radius_px * angle.cos(),
                center.y + radius_px * angle.sin(),
            ),
            edge_color,
        );
    }
    // Fan triangles: centre + two consecutive outer vertices
    for i in 0..SEGMENTS {
        mesh.indices
            .extend_from_slice(&[0, 1 + i, 1 + (i + 1) % SEGMENTS]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Upward-pointing equilateral triangle centred at `centre`.
fn draw_triangle(painter: &egui::Painter, centre: Pos2, half_size: f32, color: Color32) {
    let top = pos2(centre.x, centre.y - half_size);
    let bl = pos2(centre.x - half_size * 0.866, centre.y + half_size * 0.5);
    let br = pos2(centre.x + half_size * 0.866, centre.y + half_size * 0.5);
    painter.add(egui::Shape::convex_polygon(
        vec![top, br, bl],
        color,
        Stroke::NONE,
    ));
}

/// Node name label with a dark semi-transparent background for readability.
fn draw_label(painter: &egui::Painter, anchor: Pos2, text: &str, radius: f32) {
    let font = FontId::proportional(10.0);
    let galley = painter.layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let text_size = galley.size();
    let label_pos = pos2(anchor.x - text_size.x / 2.0, anchor.y + radius + 3.0);
    let bg = egui::Rect::from_min_size(
        pos2(label_pos.x - 2.0, label_pos.y - 1.0),
        egui::Vec2::new(text_size.x + 4.0, text_size.y + 2.0),
    );
    painter.rect_filled(bg, 2.0, LABEL_BG);
    painter.galley(label_pos, galley, Color32::WHITE);
}
