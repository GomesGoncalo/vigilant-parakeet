use crate::api::Snapshot;
use egui::{pos2, Color32, FontId, Pos2, Stroke};
use walkers::{lon_lat, MapMemory, Plugin, Position, Projector};

const OBU_COLOR: Color32 = Color32::from_rgb(30, 100, 220);
const RSU_COLOR: Color32 = Color32::from_rgb(220, 120, 30);
const SERVER_COLOR: Color32 = Color32::from_rgb(140, 140, 140);
const UPSTREAM_LINE: Color32 = Color32::from_rgba_premultiplied(80, 180, 80, 180);
const LABEL_BG: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 160);

/// walkers [`Plugin`] that draws every node in the current [`Snapshot`].
pub struct NodesPlugin {
    snapshot: Snapshot,
}

impl NodesPlugin {
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            snapshot: snapshot.clone(),
        }
    }
}

impl Plugin for NodesPlugin {
    fn run(
        self: Box<Self>,
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

        // --- Pass 1: upstream routing lines (rendered below node symbols) ---
        for (name, info) in &self.snapshot.node_info {
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
            painter.line_segment([a, b], Stroke::new(1.0, UPSTREAM_LINE));
        }

        // --- Pass 2: node symbols ---
        for (name, pos) in &self.snapshot.positions {
            let node_type = self
                .snapshot
                .node_info
                .get(name)
                .map(|n| n.node_type.as_str())
                .unwrap_or("Obu");

            let screen = to_screen(pos.lat, pos.lon);

            match node_type {
                "Rsu" => {
                    draw_triangle(painter, screen, rsu_half, RSU_COLOR);
                }
                "Server" => {
                    let half = rsu_half * 0.9;
                    let rect = egui::Rect::from_center_size(screen, egui::Vec2::splat(half * 2.0));
                    painter.rect_filled(rect, 2.0, SERVER_COLOR);
                    painter.rect_stroke(
                        rect,
                        2.0,
                        Stroke::new(1.5, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                _ => {
                    // OBU — filled circle with a thin white border.
                    painter.circle_filled(screen, obu_r, OBU_COLOR);
                    painter.circle_stroke(screen, obu_r, Stroke::new(1.0, Color32::WHITE));

                    // Bearing arrow when the vehicle is moving.
                    if pos.speed > 0.5 {
                        let angle = pos.bearing.to_radians() as f32;
                        let tip = pos2(
                            screen.x + angle.sin() * (obu_r + 6.0),
                            screen.y - angle.cos() * (obu_r + 6.0),
                        );
                        painter.line_segment([screen, tip], Stroke::new(1.5, Color32::WHITE));
                    }
                }
            }

            // Labels only when zoomed in enough.
            if obu_r > 6.0 {
                draw_label(painter, screen, name, obu_r);
            }
        }
    }
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
