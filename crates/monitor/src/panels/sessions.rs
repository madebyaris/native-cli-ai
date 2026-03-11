//! Panel: list of active and recent sessions.

use crate::session_index::IndexedSession;
use eframe::egui;

pub fn show(
    ui: &mut egui::Ui,
    sessions: &[IndexedSession],
    selected_session_id: Option<&str>,
) -> Option<String> {
    let mut selected = None;
    if sessions.is_empty() {
        ui.label("No sessions found.");
        return None;
    }

    for session in sessions {
        let label = format!(
            "{} {} {}",
            &session.id[..session.id.len().min(16)],
            session.status_display(),
            session.updated_display()
        );
        let is_selected = selected_session_id == Some(session.id.as_str());
        if ui.selectable_label(is_selected, label).clicked() {
            selected = Some(session.id.clone());
        }
    }
    selected
}
