mod app;
mod controller;
mod workspaces;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("nca desktop"),
        ..Default::default()
    };

    eframe::run_native(
        "nca-monitor",
        options,
        Box::new(|_cc| Ok(Box::new(app::DesktopApp::new()))),
    )
}
