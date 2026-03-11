//! Panel: raw event log viewer.

use super::truncate_chars;
use crate::state::TimelineEntry;
use eframe::egui;

/// Renders the event log / timeline.
pub fn show(ui: &mut egui::Ui, timeline: &[TimelineEntry]) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, entry) in timeline.iter().enumerate() {
                let (label, detail) = match entry {
                    TimelineEntry::SessionStarted {
                        session_id,
                        model,
                        workspace,
                    } => (
                        "Session started",
                        format!("{session_id} | {model} | {workspace}"),
                    ),
                    TimelineEntry::Message { role, content } => {
                        (role.as_str(), truncate_chars(content, 200))
                    }
                    TimelineEntry::Tokens { delta } => ("tokens", truncate_chars(delta, 80)),
                    TimelineEntry::ToolStart {
                        call_id,
                        tool,
                        input,
                    } => (
                        "tool start",
                        format!(
                            "{call_id} | {tool} | {}",
                            truncate_chars(&input.to_string(), 100)
                        ),
                    ),
                    TimelineEntry::ToolComplete { call_id, output } => {
                        ("tool done", format!("{call_id} | ok={}", output.success))
                    }
                    TimelineEntry::ApprovalRequested {
                        call_id,
                        tool,
                        description,
                    } => ("approval", format!("{call_id} | {tool}: {description}")),
                    TimelineEntry::ApprovalResolved { call_id, approved } => {
                        ("approval resolved", format!("{call_id} | {approved}"))
                    }
                    TimelineEntry::Cost {
                        input_tokens,
                        output_tokens,
                        estimated_cost_usd,
                    } => (
                        "cost",
                        format!("in={input_tokens} out={output_tokens} ${estimated_cost_usd:.4}"),
                    ),
                    TimelineEntry::Checkpoint {
                        phase,
                        detail,
                        turn,
                    } => ("checkpoint", format!("turn {turn} | {phase}: {detail}")),
                    TimelineEntry::SessionEnded { reason } => {
                        ("session ended", format!("{:?}", reason))
                    }
                    TimelineEntry::Error { message } => ("error", truncate_chars(message, 300)),
                    TimelineEntry::ChildSpawned {
                        child_session_id,
                        task,
                    } => (
                        "child spawned",
                        format!("{child_session_id} | {}", truncate_chars(task, 100)),
                    ),
                    TimelineEntry::ChildCompleted {
                        child_session_id,
                        status,
                    } => ("child done", format!("{child_session_id} | {status}")),
                };
                ui.horizontal(|ui| {
                    ui.label(format!("[{i}]"));
                    ui.colored_label(color_for_label(label), label);
                    ui.label(detail);
                });
            }
        });
}

fn color_for_label(label: &str) -> egui::Color32 {
    match label {
        "error" => egui::Color32::RED,
        "approval" | "approval resolved" => egui::Color32::from_rgb(200, 150, 0),
        "tool start" | "tool done" => egui::Color32::from_rgb(0, 150, 200),
        "Session started" | "session ended" => egui::Color32::from_rgb(100, 150, 100),
        _ => egui::Color32::GRAY,
    }
}
