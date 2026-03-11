//! Changed files browser panel for reviewing agent modifications.

use eframe::egui;
use std::path::PathBuf;

/// A changed file entry for display.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub change_type: String,
}

/// Show the changed files panel. Returns the path of the file the user clicked on.
pub fn show(ui: &mut egui::Ui, files: &[FileEntry], selected: Option<&PathBuf>) -> Option<PathBuf> {
    let mut clicked = None;

    if files.is_empty() {
        ui.label("No changed files.");
        return None;
    }

    ui.label(format!("{} files changed", files.len()));
    ui.separator();

    egui::ScrollArea::vertical()
        .max_height(400.0)
        .show(ui, |ui| {
            for file in files {
                let is_selected = selected == Some(&file.path);
                let color = match file.change_type.as_str() {
                    "A" => egui::Color32::from_rgb(80, 200, 120),
                    "D" => egui::Color32::from_rgb(220, 80, 80),
                    "M" => egui::Color32::from_rgb(200, 180, 60),
                    "R" => egui::Color32::from_rgb(100, 160, 220),
                    _ => egui::Color32::GRAY,
                };

                ui.horizontal(|ui| {
                    ui.colored_label(color, &file.change_type);
                    let label = file.path.display().to_string();
                    if ui.selectable_label(is_selected, &label).clicked() {
                        clicked = Some(file.path.clone());
                    }
                });
            }
        });

    clicked
}
