//! Panel: token usage, cost tracking, model info.

use crate::state::SessionVm;
use eframe::egui;

/// Renders the stats panel.
pub fn show(ui: &mut egui::Ui, vm: &SessionVm, status: &str, model: &str) {
    ui.horizontal(|ui| {
        ui.label("Status:");
        ui.label(status);
    });
    ui.horizontal(|ui| {
        ui.label("Model:");
        ui.label(model);
    });
    if let Some(action) = vm.current_action.as_ref() {
        ui.horizontal(|ui| {
            ui.label("Current action:");
            ui.label(crate::panels::truncate_chars(action, 120));
        });
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Input tokens:");
        ui.label(format!("{}", vm.input_tokens));
    });
    ui.horizontal(|ui| {
        ui.label("Output tokens:");
        ui.label(format!("{}", vm.output_tokens));
    });
    ui.horizontal(|ui| {
        ui.label("Est. cost:");
        ui.label(format!("${:.4}", vm.estimated_cost_usd));
    });
    if let Some(reason) = &vm.end_reason {
        ui.separator();
        ui.label(format!("End: {:?}", reason));
    }
}
