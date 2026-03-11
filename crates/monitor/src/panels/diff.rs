//! Diff viewer panel for reviewing changes made by the agent.

use eframe::egui;

/// Show a unified diff with syntax coloring for additions/deletions.
pub fn show(ui: &mut egui::Ui, diff_text: &str, file_path: &str) {
    if diff_text.is_empty() {
        ui.label(format!("No diff available for {file_path}"));
        return;
    }

    ui.heading(file_path);
    ui.separator();

    egui::ScrollArea::both()
        .max_height(500.0)
        .show(ui, |ui| {
            for line in diff_text.lines() {
                let (color, prefix) = if line.starts_with('+') && !line.starts_with("+++") {
                    (egui::Color32::from_rgb(80, 200, 120), true)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    (egui::Color32::from_rgb(220, 80, 80), true)
                } else if line.starts_with("@@") {
                    (egui::Color32::from_rgb(100, 160, 220), false)
                } else if line.starts_with("diff ") || line.starts_with("index ") {
                    (egui::Color32::GRAY, false)
                } else {
                    (egui::Color32::from_rgb(200, 200, 200), false)
                };

                let _ = prefix;
                ui.colored_label(color, egui::RichText::new(line).monospace());
            }
        });
}

/// Show a summary of changes: additions, deletions, and file count.
pub fn show_summary(ui: &mut egui::Ui, diff_text: &str) {
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for line in diff_text.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }

    ui.horizontal(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(80, 200, 120),
            format!("+{additions}"),
        );
        ui.colored_label(
            egui::Color32::from_rgb(220, 80, 80),
            format!("-{deletions}"),
        );
    });
}
