mod app;
mod controller;
mod ingest;
mod ipc_client;
mod panels;
mod session_index;
mod state;
mod workspaces;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("nca monitor"),
        ..Default::default()
    };

    eframe::run_native(
        "nca-monitor",
        options,
        Box::new(|_cc| Ok(Box::new(app::MonitorApp::new()))),
    )
}
