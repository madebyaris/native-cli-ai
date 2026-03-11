//! Panel: tool call history with inputs and outputs.

use super::truncate_chars;
use crate::state::ToolCallState;
use eframe::egui;
use std::collections::HashMap;

/// Renders the tool inspector panel.
pub fn show(ui: &mut egui::Ui, tool_calls: &HashMap<String, ToolCallState>) {
    if tool_calls.is_empty() {
        ui.label("No tool calls yet.");
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (call_id, state) in tool_calls {
                egui::CollapsingHeader::new(format!("{}", call_id))
                    .default_open(false)
                    .show(ui, |ui| match state {
                        ToolCallState::Started {
                            tool,
                            input,
                            output,
                        } => {
                            ui.label(format!("Tool: {tool}"));
                            ui.label("Input:");
                            ui.code(truncate_chars(&input.to_string(), 500));
                            if let Some(o) = output {
                                ui.label("Output:");
                                ui.code(truncate_chars(&o.output, 300));
                                if let Some(e) = &o.error {
                                    ui.colored_label(egui::Color32::RED, e);
                                }
                            }
                        }
                        ToolCallState::Completed { output } => {
                            ui.label(format!("Tool: completed | ok={}", output.success));
                            ui.code(truncate_chars(&output.output, 500));
                            if let Some(e) = &output.error {
                                ui.colored_label(egui::Color32::RED, e);
                            }
                        }
                    });
            }
        });
}
