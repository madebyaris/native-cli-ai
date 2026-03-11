use eframe::egui;

pub struct MonitorApp {
    selected_session: Option<String>,
}

impl MonitorApp {
    pub fn new() -> Self {
        Self {
            selected_session: None,
        }
    }
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("sessions_panel")
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.heading("Sessions");
                ui.separator();
                ui.label("No active sessions");
                // TODO: list sessions from IPC discovery
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(id) = &self.selected_session {
                ui.heading(format!("Session: {id}"));
            } else {
                ui.heading("nca monitor");
                ui.label("Select a session from the sidebar to begin.");
            }
            // TODO: show terminal mirror, tool calls, diffs, stats panels
        });
    }
}
