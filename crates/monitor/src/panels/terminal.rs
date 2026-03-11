//! Panel: live terminal output mirror for the selected session.

use crate::state::{SessionVm, TimelineEntry};
use eframe::egui;

pub fn show(ui: &mut egui::Ui, vm: &SessionVm) {
    if vm.timeline.is_empty() {
        ui.label("No live output yet.");
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for line in render_terminal_lines(vm) {
                ui.monospace(line);
            }
        });
}

fn render_terminal_lines(vm: &SessionVm) -> Vec<String> {
    let mut lines = Vec::new();
    let mut token_buffer = String::new();

    for entry in &vm.timeline {
        match entry {
            TimelineEntry::Tokens { delta } => {
                token_buffer.push_str(delta);
                if token_buffer.len() > 300 {
                    lines.push(format!("assistant> {token_buffer}"));
                    token_buffer.clear();
                }
            }
            TimelineEntry::ToolStart { tool, .. } => {
                flush_tokens(&mut token_buffer, &mut lines);
                lines.push(format!("tool:start {tool}"));
            }
            TimelineEntry::ToolComplete { output, .. } => {
                flush_tokens(&mut token_buffer, &mut lines);
                if output.success {
                    lines.push("tool:done ok".to_string());
                } else {
                    lines.push(format!(
                        "tool:done error {}",
                        output.error.as_deref().unwrap_or("unknown")
                    ));
                }
            }
            TimelineEntry::ApprovalRequested { tool, .. } => {
                flush_tokens(&mut token_buffer, &mut lines);
                lines.push(format!("approval:waiting {tool}"));
            }
            TimelineEntry::ApprovalResolved { approved, .. } => {
                flush_tokens(&mut token_buffer, &mut lines);
                lines.push(format!("approval:resolved {approved}"));
            }
            TimelineEntry::Error { message } => {
                flush_tokens(&mut token_buffer, &mut lines);
                lines.push(format!("error: {message}"));
            }
            TimelineEntry::Message { role, content } if role == "assistant" => {
                flush_tokens(&mut token_buffer, &mut lines);
                lines.push(format!("assistant> {content}"));
            }
            _ => {}
        }
    }

    flush_tokens(&mut token_buffer, &mut lines);
    lines
}

fn flush_tokens(buffer: &mut String, lines: &mut Vec<String>) {
    if !buffer.is_empty() {
        lines.push(format!("assistant> {buffer}"));
        buffer.clear();
    }
}
