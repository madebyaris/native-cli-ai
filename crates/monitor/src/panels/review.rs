//! Review workbench panel: combines changed files, diff viewer, and merge actions.

use eframe::egui;
use std::path::PathBuf;

use super::diff;
use super::files::{self, FileEntry};

/// State for the review workbench.
#[derive(Debug, Default)]
pub struct ReviewState {
    pub changed_files: Vec<FileEntry>,
    pub selected_file: Option<PathBuf>,
    pub current_diff: String,
    pub worktree_path: Option<PathBuf>,
    pub base_branch: Option<String>,
    pub branch_name: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub merge_status: Option<String>,
}

impl ReviewState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Show the full review workbench.
pub fn show(ui: &mut egui::Ui, state: &mut ReviewState) -> ReviewAction {
    let mut action = ReviewAction::None;

    if state.worktree_path.is_none() {
        ui.label("No worktree associated with this session.");
        ui.label("Start a session with isolated worktree to enable review.");
        return action;
    }

    // Header with branch info
    ui.horizontal(|ui| {
        if let Some(ref branch) = state.branch_name {
            ui.label(format!("Branch: {branch}"));
        }
        if let Some(ref base) = state.base_branch {
            ui.label(format!("\u{2190} {base}"));
        }
        ui.separator();
        ui.label(format!(
            "{} ahead, {} behind",
            state.ahead, state.behind
        ));
    });

    if let Some(ref msg) = state.merge_status {
        ui.colored_label(egui::Color32::from_rgb(200, 180, 60), msg);
    }

    ui.separator();

    // Split: files on left, diff on right
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(200.0);
            ui.heading("Changed Files");
            if let Some(clicked) = files::show(ui, &state.changed_files, state.selected_file.as_ref()) {
                state.selected_file = Some(clicked);
                action = ReviewAction::LoadDiff;
            }
        });

        ui.separator();

        ui.vertical(|ui| {
            if let Some(ref file_path) = state.selected_file {
                diff::show(ui, &state.current_diff, &file_path.display().to_string());
            } else {
                ui.label("Select a file to view its diff.");
            }
        });
    });

    ui.add_space(8.0);
    ui.separator();

    // Merge actions
    ui.horizontal(|ui| {
        if ui.button("Accept & Merge").clicked() {
            action = ReviewAction::Merge;
        }
        if ui.button("Discard Changes").clicked() {
            action = ReviewAction::Discard;
        }
        if ui.button("Refresh").clicked() {
            action = ReviewAction::Refresh;
        }
    });

    action
}

/// Actions the review panel can request from the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAction {
    None,
    LoadDiff,
    Merge,
    Discard,
    Refresh,
}
