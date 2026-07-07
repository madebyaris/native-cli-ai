//! NcaModel-based event loop replacing run_blocking.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::feedback::TuiFeedbackMsg;
use super::model::{NcaModel, SideEffectChannels};
use super::msg::Msg;
use crate::tui::app::{ApprovalAnswer, restore_terminal, setup_terminal};
use nca_common::event::{InteractiveQuestionPayload, QuestionSelection};

/// Parameters for initializing the NcaModel event loop.
pub(crate) struct NcaModelParams {
    pub session_id: String,
    pub model: String,
    pub agent_label: String,
    pub permission_mode: String,
    pub workspace_root: std::path::PathBuf,
    pub skill_dirs: Vec<std::path::PathBuf>,
    pub plugin_commands: Vec<(String, Vec<String>)>,
}

/// Run the NcaModel-based TUI event loop.
///
/// This is the replacement for `app.rs::run_blocking()`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_nca_model(
    feedback_rx: tokio::sync::mpsc::UnboundedReceiver<TuiFeedbackMsg>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Msg>,
    question_answer_tx: Option<
        tokio::sync::mpsc::UnboundedSender<(String, nca_common::event::QuestionSelection)>,
    >,
    approval_answer_tx: Option<tokio::sync::mpsc::UnboundedSender<ApprovalAnswer>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    active_question_id: Arc<StdMutex<Option<String>>>,
    active_question_payload: Arc<StdMutex<Option<InteractiveQuestionPayload>>>,
    active_approval_payload: Arc<StdMutex<Option<crate::tui::state::ApprovalRequest>>>,
    staged_images: Arc<StdMutex<Vec<nca_common::message::ImageAttachment>>>,
    params: NcaModelParams,
) -> anyhow::Result<()> {
    // Setup terminal
    let mut terminal = setup_terminal()?;

    // Create NcaModel
    let mut nca_model = NcaModel::new(
        feedback_rx,
        cmd_tx,
        SideEffectChannels {
            question_answer_tx,
            approval_answer_tx,
            cancel_flag,
            active_question_id,
            active_question_payload,
            active_approval_payload,
            staged_images,
        },
    );

    // Initialize composer with slash entries and workspace files
    nca_model
        .components
        .composer
        .state_mut()
        .load_slash_entries(
            &params.workspace_root,
            &params.skill_dirs,
            &params.plugin_commands,
        );
    nca_model
        .components
        .composer
        .state_mut()
        .load_workspace_files(&params.workspace_root);

    // Initialize status bar
    nca_model
        .components
        .status_bar
        .update_session(&params.session_id, &params.model);
    nca_model
        .components
        .status_bar
        .update_agent_profile(&params.agent_label);
    nca_model
        .components
        .status_bar
        .update_permission_mode(&params.permission_mode);

    // Main event loop
    let result = loop {
        nca_model.tick(&mut terminal)?;
        if nca_model.quit {
            break Ok(());
        }
    };

    // Restore terminal (always, even on error)
    restore_terminal();

    result
}
