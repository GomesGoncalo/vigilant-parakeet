mod api;
mod app;
mod nodes_plugin;

fn main() -> eframe::Result<()> {
    let base_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://localhost:3030".to_string());
    let base_url = base_url.trim_end_matches('/').to_string();

    let (tx, rx) = std::sync::mpsc::sync_channel::<api::Snapshot>(4);

    std::thread::spawn(move || api::poll_loop(base_url, tx));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vigilant-parakeet — live map")
            .with_inner_size([1400.0, 900.0]),
        ..Default::default()
    };

    eframe::run_native(
        "vigilant-parakeet",
        native_options,
        Box::new(|cc| Ok(Box::new(app::VizApp::new(cc, rx)))),
    )
}
