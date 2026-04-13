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
    /// Whether to follow the selected OBU.
    follow: bool,
    /// Current text in the search box.
    search_query: String,
    /// Node name that is currently highlighted (jumped to via search).
    highlighted_node: Option<String>,
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
            follow: false,
            search_query: String::new(),
            highlighted_node: None,
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
            .request_repaint_after(std::time::Duration::from_millis(20));

        // Auto-fit to node centroid on the first non-empty positions batch.
        if !self.fitted && !self.snapshot.positions.is_empty() {
            let (clat, clon) = centroid(&self.snapshot.positions);
            self.map_memory.center_at(lon_lat(clon, clat));
            let _ = self.map_memory.set_zoom(13.0);
            self.fitted = true;
        }

        let jump_to: Option<String> = egui::Panel::right("info_panel")
            .resizable(false)
            .exact_size(200.0)
            .show_inside(ui, |ui| {
                draw_sidebar(
                    ui,
                    &self.snapshot,
                    &mut self.tile_opacity,
                    &mut self.show_rsu_range,
                    &mut self.follow,
                    &mut self.search_query,
                    &mut self.highlighted_node,
                )
            })
            .inner;

        // Jump: center map on the matched node's position.
        if let Some(ref name) = jump_to {
            if let Some(pos) = self.snapshot.positions.get(name) {
                self.map_memory.center_at(lon_lat(pos.lon, pos.lat));
            }
        }

        // If follow is enabled, always center on the highlighted node's position.
        if self.follow {
            if let Some(ref name) = self.highlighted_node {
                if let Some(pos) = self.snapshot.positions.get(name) {
                    self.map_memory.center_at(lon_lat(pos.lon, pos.lat));
                }
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                let my_pos = lon_lat(-8.625, 41.157);
                let map = Map::new(None, &mut self.map_memory, my_pos)
                    .with_layer(&mut self.tiles, self.tile_opacity)
                    .with_plugin(NodesPlugin::new(
                        &self.snapshot,
                        self.show_rsu_range,
                        self.highlighted_node.clone(),
                    ));

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

/// Returns the node name to jump to (if the user confirmed a search), or `None`.
fn draw_sidebar(
    ui: &mut egui::Ui,
    snap: &Snapshot,
    tile_opacity: &mut f32,
    show_rsu_range: &mut bool,
    follow: &mut bool,
    search_query: &mut String,
    highlighted_node: &mut Option<String>,
) -> Option<String> {
    let mut jump_to: Option<String> = None;

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

    // Search
    ui.label("Find node");
    let query_lower = search_query.to_lowercase();
    let matches: Vec<String> = if query_lower.is_empty() {
        vec![]
    } else {
        let mut v: Vec<String> = snap
            .positions
            .keys()
            .filter(|n| n.to_lowercase().contains(&query_lower))
            .cloned()
            .collect();
        v.sort_unstable();
        v
    };

    let _search_response = ui.add(
        egui::TextEdit::singleline(search_query)
            .hint_text("node name…")
            .desired_width(f32::INFINITY),
    );

    // Clear button + match count on the same row.
    ui.horizontal(|ui| {
        if ui.small_button("✕").clicked() {
            search_query.clear();
            *highlighted_node = None;
        }
        match matches.len() {
            0 if !search_query.is_empty() => {
                ui.label(
                    egui::RichText::new("no match")
                        .small()
                        .color(egui::Color32::YELLOW),
                );
            }
            1 => {
                ui.label(
                    egui::RichText::new("1 match")
                        .small()
                        .color(egui::Color32::GREEN),
                );
            }
            n if n > 1 => {
                ui.label(egui::RichText::new(format!("{n} matches")).small());
            }
            _ => {}
        }
    });

    // Follow toggle
    let follow_label = if *follow {
        "Unfollow selected node"
    } else {
        "Follow selected node"
    };
    if ui.button(follow_label).clicked() {
        *follow = !*follow;
    }

    if search_query.is_empty() {
        *highlighted_node = None;
        jump_to = None;
    }

    // Show a scrollable list when there are multiple matches.
    egui::ScrollArea::vertical()
        .max_height(120.0)
        .show(ui, |ui| {
            for name in &matches {
                let selected = highlighted_node.as_deref() == Some(name.as_str());
                if ui.selectable_label(selected, name).clicked() {
                    *highlighted_node = Some(name.clone());
                    if *follow {
                        jump_to = Some(name.clone());
                    }
                }
            }
        });

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

    ui.separator();

    if let Some(t) = highlighted_node.as_ref() {
        if let Some(info) = snap.node_info.get(t) {
            ui.label(
                egui::RichText::new(format!("Type: {}", info.node_type))
                    .small()
                    .color(egui::Color32::WHITE),
            );
            ui.label(
                egui::RichText::new(format!("mac: {}", info.mac))
                    .small()
                    .color(egui::Color32::WHITE),
            );
            if let Some(ip) = &info.virtual_ip {
                ui.label(
                    egui::RichText::new(format!("local ip: {}", ip))
                        .small()
                        .color(egui::Color32::WHITE),
                );
            }
            if let Some(upstream) = &info.upstream {
                ui.label(
                    egui::RichText::new(format!("upstream mac: {}", upstream.mac))
                        .small()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(format!("upstream hops: {}", upstream.hops))
                        .small()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "upstream rssi: {}",
                        upstream
                            .rssi_dbm
                            .map_or("N/A".to_string(), |l| format!("{} dbm", l))
                    ))
                    .small()
                    .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "latency: {}",
                        upstream
                            .latency_us
                            .map_or("N/A".to_string(), |l| format!("{} us", l))
                    ))
                    .small()
                    .color(egui::Color32::WHITE),
                );
            }
            ui.label(
                egui::RichText::new(format!("has session: {}", info.has_session))
                    .small()
                    .color(egui::Color32::WHITE),
            );
        }
    }

    jump_to
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
