//! Panel: condensed timeline / event feed.

use super::truncate_chars;
use crate::state::TimelineEntry;
use eframe::egui;

/// Renders a condensed timeline of key events.
pub fn show(ui: &mut egui::Ui, timeline: &[TimelineEntry]) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in timeline {
                let text = match entry {
                    TimelineEntry::SessionStarted { model, .. } => format!("Session | {model}"),
                    TimelineEntry::Message { role, content } => {
                        format!("{role}: {}", truncate_chars(content, 120))
                    }
                    TimelineEntry::ToolStart { tool, .. } => format!("tool: {tool}"),
                    TimelineEntry::ToolComplete { output, .. } => {
                        format!("{}", if output.success { "ok" } else { "fail" })
                    }
                    TimelineEntry::ApprovalRequested { tool, .. } => format!("Approval: {tool}"),
                    TimelineEntry::ApprovalResolved { approved, .. } => {
                        if *approved {
                            "Approved".into()
                        } else {
                            "Denied".into()
                        }
                    }
                    TimelineEntry::Cost {
                        input_tokens,
                        output_tokens,
                        estimated_cost_usd,
                    } => {
                        format!("{input_tokens}+{output_tokens} tokens | ${estimated_cost_usd:.4}")
                    }
                    TimelineEntry::Checkpoint { phase, turn, .. } => {
                        format!("turn {turn}: {phase}")
                    }
                    TimelineEntry::SessionEnded { reason } => format!("End: {:?}", reason),
                    TimelineEntry::Error { message } => truncate_chars(message, 200),
                    TimelineEntry::ChildSpawned {
                        child_session_id,
                        task,
                    } => {
                        format!(
                            "child spawned: {child_session_id} - {}",
                            truncate_chars(task, 60)
                        )
                    }
                    TimelineEntry::ChildCompleted {
                        child_session_id,
                        status,
                    } => {
                        format!("child done: {child_session_id} [{status}]")
                    }
                    TimelineEntry::Tokens { .. } => continue,
                };
                ui.label(text);
            }
        });
}
