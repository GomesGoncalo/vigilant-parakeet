use crate::api::Snapshot;
use crate::nodes_plugin::NodesPlugin;
use std::sync::mpsc::Receiver;
use walkers::{lon_lat, sources::OpenStreetMap, HttpTiles, Map, MapMemory};

pub struct VizApp {
    tiles: HttpTiles,
    map_memory: MapMemory,
    snapshot: Snapshot,
    rx: Receiver<Snapshot>,
    /// True once we have auto-centred the map on the first batch of positions.
    fitted: bool,
    /// Tile layer opacity (0.0 = hidden, 1.0 = fully visible).
    tile_opacity: f32,
    /// Whether to draw the RSU coverage range circles.
    show_rsu_range: bool,
}

impl VizApp {
    pub fn new(cc: &eframe::CreationContext, rx: Receiver<Snapshot>) -> Self {
        // Default view: Porto city centre.
        let porto = lon_lat(-8.625, 41.157);
        let mut map_memory = MapMemory::default();
        map_memory.center_at(porto);
        let _ = map_memory.set_zoom(13.0);

        Self {
            tiles: HttpTiles::new(OpenStreetMap, cc.egui_ctx.clone()),
            map_memory,
            snapshot: Snapshot::default(),
            rx,
            fitted: false,
            tile_opacity: 1.0,
            show_rsu_range: false,
        }
    }
}

impl eframe::App for VizApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Drain the channel; keep only the latest snapshot.
        while let Ok(snap) = self.rx.try_recv() {
            self.snapshot = snap;
        }

        // Keep repainting at ~5 fps so movement stays visible.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));

        // Auto-fit to node centroid on the first non-empty positions batch.
        if !self.fitted && !self.snapshot.positions.is_empty() {
            let (clat, clon) = centroid(&self.snapshot.positions);
            self.map_memory.center_at(lon_lat(clon, clat));
            let _ = self.map_memory.set_zoom(13.0);
            self.fitted = true;
        }

        egui::Panel::right("info_panel")
            .resizable(false)
            .exact_size(200.0)
            .show_inside(ui, |ui| {
                draw_sidebar(ui, &self.snapshot, &mut self.tile_opacity, &mut self.show_rsu_range);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                // `my_position` is not used for tracking here because we
                // call `center_at` to detach the map immediately — it just
                // serves as the initial fallback anchor.
                let my_pos = lon_lat(-8.625, 41.157);
                let map = Map::new(None, &mut self.map_memory, my_pos)
                    .with_layer(&mut self.tiles, self.tile_opacity)
                    .with_plugin(NodesPlugin::new(&self.snapshot, self.show_rsu_range));

                map.show(ui, |_, _, _, _| ());
            });
    }
}

fn centroid(positions: &std::collections::HashMap<String, crate::api::NodePosition>) -> (f64, f64) {
    let n = positions.len() as f64;
    let (sum_lat, sum_lon) = positions
        .values()
        .fold((0.0, 0.0), |(la, lo), p| (la + p.lat, lo + p.lon));
    (sum_lat / n, sum_lon / n)
}

fn draw_sidebar(ui: &mut egui::Ui, snap: &Snapshot, tile_opacity: &mut f32, show_rsu_range: &mut bool) {
    ui.add_space(8.0);
    ui.heading("vigilant-parakeet");
    ui.separator();

    let n_obu = snap
        .node_info
        .values()
        .filter(|n| n.node_type == "Obu")
        .count();
    let n_rsu = snap
        .node_info
        .values()
        .filter(|n| n.node_type == "Rsu")
        .count();
    let n_srv = snap
        .node_info
        .values()
        .filter(|n| n.node_type == "Server")
        .count();
    let n_pos = snap.positions.len();

    egui::Grid::new("stats_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("OBUs:");
            ui.label(n_obu.to_string());
            ui.end_row();
            ui.label("RSUs:");
            ui.label(n_rsu.to_string());
            ui.end_row();
            if n_srv > 0 {
                ui.label("Servers:");
                ui.label(n_srv.to_string());
                ui.end_row();
            }
            ui.label("Positions:");
            ui.label(n_pos.to_string());
            ui.end_row();
        });

    ui.separator();

    // Map opacity control
    ui.label("Map opacity");
    ui.horizontal(|ui| {
        ui.add(
            egui::Slider::new(tile_opacity, 0.0..=1.0)
                .step_by(0.05)
                .show_value(false),
        );
        if ui
            .button(if *tile_opacity > 0.1 {
                "🌫 Dim"
            } else {
                "🗺 Show"
            })
            .clicked()
        {
            *tile_opacity = if *tile_opacity > 0.1 { 0.0 } else { 1.0 };
        }
    });

    ui.separator();

    // RSU range toggle
    let range_label = if *show_rsu_range {
        "Hide RSU range"
    } else {
        "Show RSU range"
    };
    if ui.button(range_label).clicked() {
        *show_rsu_range = !*show_rsu_range;
    }

    ui.separator();

    // Legend
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), 16.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let dot = egui::pos2(rect.min.x + 8.0, rect.center().y);
    painter.circle_filled(dot, 5.0, egui::Color32::from_rgb(30, 100, 220));
    painter.text(
        egui::pos2(dot.x + 14.0, dot.y),
        egui::Align2::LEFT_CENTER,
        "OBU",
        egui::FontId::default(),
        ui.visuals().text_color(),
    );

    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), 16.0),
        egui::Sense::hover(),
    );
    let tri_center = egui::pos2(rect.min.x + 8.0, rect.center().y);
    draw_triangle(
        ui.painter(),
        tri_center,
        6.0,
        egui::Color32::from_rgb(220, 120, 30),
    );
    ui.painter().text(
        egui::pos2(tri_center.x + 14.0, tri_center.y),
        egui::Align2::LEFT_CENTER,
        "RSU",
        egui::FontId::default(),
        ui.visuals().text_color(),
    );

    ui.separator();

    if let Some(t) = snap.last_positions_at {
        let age_ms = t.elapsed().as_millis();
        ui.label(
            egui::RichText::new(format!("Updated {age_ms} ms ago"))
                .small()
                .color(if age_ms < 1000 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::YELLOW
                }),
        );
    } else {
        ui.label(
            egui::RichText::new("Waiting for simulator…")
                .small()
                .italics(),
        );
    }
}

/// Draw an upward-pointing equilateral triangle centred at `centre`.
fn draw_triangle(
    painter: &egui::Painter,
    centre: egui::Pos2,
    half_size: f32,
    color: egui::Color32,
) {
    let top = egui::pos2(centre.x, centre.y - half_size);
    let bl = egui::pos2(centre.x - half_size * 0.866, centre.y + half_size * 0.5);
    let br = egui::pos2(centre.x + half_size * 0.866, centre.y + half_size * 0.5);
    painter.add(egui::Shape::convex_polygon(
        vec![top, br, bl],
        color,
        egui::Stroke::NONE,
    ));
}
