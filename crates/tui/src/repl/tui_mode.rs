//! Full-screen ratatui mode for the REPL.
//!
//! Keeps the stdio-mode `Repl::run` lean: this module owns everything that
//! depends on `run_blocking`, `spawn_tui_bridge`, and the `TuiCmd` dispatch
//! loop, so `repl/mod.rs` can stay readable.

use super::Repl;
use super::commands::{permission_mode_from_index, permission_mode_index};
use super::{AgentProfile, ReplOutput};
use crate::file_mentions::expand_at_file_mentions_default;
use crate::runner::{dispatch_question_answer, dispatch_tool_approval};
use crate::tui::input::ApprovalAnswer;
use crate::tui::{
    DisplayBlock, TuiCmd, TuiSessionState, git_create_branch, git_current_branch,
    git_list_branches, git_switch_branch, replay_event_log_into_state, run_blocking,
    spawn_tui_bridge,
};
use nca_common::config::{PermissionMode, ProviderKind};
use nca_common::event::{EndReason, QuestionSelection};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;

impl Repl {
    /// Full-screen TUI: transcript + streaming + composer (default on TTY).
    pub async fn run_with_tui(&mut self) -> anyhow::Result<()> {
        let session_id = self.runtime.session_id().to_string();
        let model = self.runtime.model().to_string();
        let perm = format!("{:?}", self.runtime.permission_mode());
        let tui_state: Arc<Mutex<TuiSessionState>> = Arc::new(Mutex::new(TuiSessionState::new(
            session_id,
            model,
            self.current_agent_label.clone(),
            perm,
            self.runtime.workspace_root().to_path_buf(),
        )));

        let log_path = self.runtime.event_log_path();
        replay_event_log_into_state(&log_path, &tui_state).await;

        let workspace = self.runtime.workspace_root();
        if let Some(branch) = git_current_branch(workspace)
            && let Ok(mut g) = tui_state.lock()
        {
            g.set_current_branch(&branch);
        }

        let rx = self
            .runtime
            .take_event_rx()
            .ok_or_else(|| anyhow::anyhow!("internal: event channel already taken"))?;
        let ipc = self.runtime.take_ipc_handle();
        let approval = self.runtime.take_ipc_approval_pending();
        let question = self.runtime.question_pending();
        let initial_version = tui_state.lock().map(|g| g.state_version).unwrap_or(1);
        let (version_tx, version_rx) = tokio::sync::watch::channel(initial_version);
        let _bridge = spawn_tui_bridge(
            rx,
            log_path,
            ipc,
            approval.clone(),
            question.clone(),
            tui_state.clone(),
            Some(version_tx.clone()),
        );

        let _spawn_task = {
            let spawn_rx = self.runtime.take_spawn_rx();
            let event_tx = self.runtime.event_tx();
            if let Some(srx) = spawn_rx {
                Some(nca_runtime::supervisor::spawn_subagent_consumer(
                    srx,
                    self.runtime.session_id().to_string(),
                    self.runtime.workspace_root().to_path_buf(),
                    self.runtime.config().clone(),
                    self.runtime.messages().to_vec(),
                    event_tx,
                ))
            } else {
                None
            }
        };

        // Answers must bypass the main `cmd_rx` loop: while `run_turn` is
        // blocked inside `ask_question`, that task never receives
        // `TuiCmd::Submit` or `QuestionAnswer`. Bounded at 16 because UI
        // answer/approval bursts are user-driven and inherently small.
        let (answer_tx, mut answer_rx) =
            tokio::sync::mpsc::channel::<(String, QuestionSelection)>(16);
        let qp_dispatch = question.clone();
        tokio::spawn(async move {
            while let Some((qid, sel)) = answer_rx.recv().await {
                let _ = dispatch_question_answer(&qp_dispatch, &qid, sel);
            }
        });
        let answer_for_tui = answer_tx.clone();
        drop(answer_tx);

        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::channel::<ApprovalAnswer>(16);
        let approval_dispatch = approval.clone();
        let approval_state = tui_state.clone();
        tokio::spawn(async move {
            while let Some(answer) = approval_rx.recv().await {
                let (call_id, verdict) = match answer {
                    ApprovalAnswer::Verdict { call_id, approved } => (
                        call_id,
                        if approved {
                            nca_core::approval::ApprovalVerdict::Approved
                        } else {
                            nca_core::approval::ApprovalVerdict::Denied
                        },
                    ),
                    ApprovalAnswer::AllowPattern { call_id, pattern } => (
                        call_id,
                        nca_core::approval::ApprovalVerdict::AllowPattern(pattern),
                    ),
                };
                if !dispatch_tool_approval(&approval_dispatch, &call_id, verdict)
                    && let Ok(mut g) = approval_state.lock()
                {
                    g.clear_active_approval_if_matches(&call_id);
                    g.push_error(
                        "approval was no longer pending; cleared stale approval state".into(),
                    );
                }
            }
        });
        let approval_for_tui = approval_tx.clone();
        drop(approval_tx);

        // User-driven `TuiCmd` traffic (submit, picker actions, /commands) is
        // always small; 64 is plenty and preserves backpressure.
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<TuiCmd>(64);
        let st = tui_state.clone();
        let banner = self.run_mode;
        let cancel_flag = self.runtime.cancel_handle();
        let prewarm = (
            tokio::runtime::Handle::current(),
            self.runtime.config().clone(),
        );
        let version_tx_for_ui = version_tx.clone();
        let ui = tokio::task::spawn_blocking(move || {
            run_blocking(
                st,
                version_rx,
                version_tx_for_ui,
                cmd_tx,
                Some(answer_for_tui),
                Some(approval_for_tui),
                banner,
                Some(cancel_flag),
                Some(prewarm),
            )
        });

        loop {
            let cmd = cmd_rx.recv().await;
            let Some(cmd) = cmd else { break };
            match cmd {
                TuiCmd::Exit => {
                    if let Ok(mut g) = tui_state.lock() {
                        g.should_exit = true;
                    }
                    break;
                }
                TuiCmd::CycleAgent => {
                    let next = self.agent_profile.next();
                    self.agent_profile = next;
                    self.current_agent_label = format!("@{}", next.label());
                    if next == AgentProfile::Plan {
                        self.runtime.set_permission_mode(PermissionMode::Plan);
                    } else {
                        self.runtime.set_permission_mode(PermissionMode::Default);
                    }
                    if let Ok(mut g) = tui_state.lock() {
                        g.set_agent_profile(&self.current_agent_label);
                        g.set_permission_mode(&format!("{:?}", self.runtime.permission_mode()));
                    }
                }
                TuiCmd::CancelTurn => {
                    self.runtime.request_cancel();
                }
                TuiCmd::OpenBranchPicker => {
                    let workspace = self.runtime.workspace_root();
                    let branches = git_list_branches(workspace);
                    let current = git_current_branch(workspace).unwrap_or_default();
                    if let Ok(mut g) = tui_state.lock() {
                        g.open_branch_picker(branches, &current);
                        g.set_current_branch(&current);
                    }
                }
                TuiCmd::SwitchBranch(name) => {
                    let workspace = self.runtime.workspace_root();
                    if git_switch_branch(workspace, &name) {
                        if let Ok(mut g) = tui_state.lock() {
                            g.set_current_branch(&name);
                            g.blocks.push(DisplayBlock::System(format!(
                                "Switched to branch: {}",
                                name
                            )));
                        }
                    } else if let Ok(mut g) = tui_state.lock() {
                        g.push_error(format!("Failed to switch to branch: {}", name));
                    }
                }
                TuiCmd::CreateBranch(name) => {
                    let workspace = self.runtime.workspace_root();
                    if git_create_branch(workspace, &name) {
                        if let Ok(mut g) = tui_state.lock() {
                            g.set_current_branch(&name);
                            g.blocks.push(DisplayBlock::System(format!(
                                "Created and switched to branch: {}",
                                name
                            )));
                        }
                    } else if let Ok(mut g) = tui_state.lock() {
                        g.push_error(format!("Failed to create branch: {}", name));
                    }
                }
                TuiCmd::ApplyDefaultProvider(p) => {
                    if p == ProviderKind::Custom
                        && self
                            .runtime
                            .config()
                            .provider
                            .custom
                            .base_url
                            .trim()
                            .is_empty()
                    {
                        if let Ok(mut g) = tui_state.lock() {
                            g.open_custom_provider_setup(self.runtime.model().to_string());
                            g.blocks.push(DisplayBlock::System(
                                "[provider] add custom provider wizard opened".into(),
                            ));
                        }
                    } else {
                        self.apply_provider_in_session(p, ReplOutput::Tui(&tui_state))
                            .await?;
                    }
                }
                TuiCmd::ApplyCustomProviderSetup {
                    compatibility,
                    base_url,
                    api_key,
                    model,
                } => {
                    self.persist_custom_provider_config(
                        compatibility,
                        base_url,
                        Some(api_key),
                        Some(model),
                        ReplOutput::Tui(&tui_state),
                    )
                    .await?;
                    if let Ok(mut g) = tui_state.lock() {
                        g.blocks.push(DisplayBlock::System(
                            "[custom] provider saved and set as default".into(),
                        ));
                    }
                }
                TuiCmd::PromptApiKey(p, connect_after_save) => {
                    if let Ok(mut g) = tui_state.lock() {
                        if p == ProviderKind::Custom
                            && self
                                .runtime
                                .config()
                                .provider
                                .custom
                                .base_url
                                .trim()
                                .is_empty()
                        {
                            g.open_custom_provider_setup(self.runtime.model().to_string());
                            g.blocks.push(DisplayBlock::System(
                                "[apikey] configure custom endpoint first (wizard opened)".into(),
                            ));
                        } else {
                            g.open_api_key_modal(
                                p,
                                self.runtime.config().provider.api_key_present_for(p),
                                connect_after_save,
                            );
                        }
                    }
                }
                TuiCmd::ApplyModel(model_name) => {
                    let resolved = self.runtime.config().model.resolve_alias(&model_name);
                    let mut cfg = self.runtime.config().clone();
                    cfg.apply_model_override(&resolved);
                    cfg.model.track_recent_model(&resolved);
                    match self.runtime.apply_nca_config(cfg) {
                        Ok(()) => {
                            if let Err(e) = self
                                .runtime
                                .config()
                                .save_workspace_file(self.runtime.workspace_root())
                            {
                                if let Ok(mut g) = tui_state.lock() {
                                    g.push_error(format!("[model] workspace save failed: {e}"));
                                }
                            } else if let Ok(mut g) = tui_state.lock() {
                                g.model = self.runtime.model().to_string();
                                g.blocks.push(DisplayBlock::System(format!(
                                    "[model] switched to {} (saved)",
                                    self.runtime.model()
                                )));
                            }
                        }
                        Err(e) => {
                            if let Ok(mut g) = tui_state.lock() {
                                g.push_error(format!("[model] {e}"));
                            }
                        }
                    }
                }
                TuiCmd::ApplyModelProvider(p) => {
                    self.apply_provider_in_session(p, ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::ApplyPermission(idx) => {
                    let mode = permission_mode_from_index(idx);
                    self.runtime.set_permission_mode(mode);
                    if let Ok(mut g) = tui_state.lock() {
                        g.set_permission_mode(&format!("{mode:?}"));
                        g.blocks.push(DisplayBlock::System(format!(
                            "permission mode set to {mode:?}"
                        )));
                    }
                }
                TuiCmd::SwitchAgent(idx) => {
                    if let Some(&profile) = AgentProfile::ALL.get(idx) {
                        self.agent_profile = profile;
                        self.current_agent_label = format!("@{}", profile.label());
                        if profile == AgentProfile::Plan {
                            self.runtime.set_permission_mode(PermissionMode::Plan);
                        } else {
                            self.runtime.set_permission_mode(PermissionMode::Default);
                        }
                        if let Ok(mut g) = tui_state.lock() {
                            g.set_agent_profile(&self.current_agent_label);
                            g.set_permission_mode(&format!("{:?}", self.runtime.permission_mode()));
                            g.blocks.push(DisplayBlock::System(format!(
                                "switched to @{}",
                                profile.label()
                            )));
                        }
                    }
                }
                TuiCmd::OpenEditor => {
                    self.handle_command("/editor", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::NewSession => {
                    self.handle_command("/new", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::RunCompact => {
                    self.handle_command("/compact", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::OpenModelPicker => {
                    self.handle_command("/models", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::OpenStatus => {
                    self.handle_command("/status", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::OpenHelp => {
                    self.handle_command("/help", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::OpenAgentPicker => {
                    let current_idx = AgentProfile::ALL
                        .iter()
                        .position(|p| *p == self.agent_profile)
                        .unwrap_or(0);
                    if let Ok(mut g) = tui_state.lock() {
                        g.open_agent_picker(current_idx);
                    }
                }
                TuiCmd::OpenPermissionPicker => {
                    let current_idx = permission_mode_index(self.runtime.permission_mode());
                    if let Ok(mut g) = tui_state.lock() {
                        g.open_permission_picker(current_idx);
                    }
                }
                TuiCmd::OpenSessions => {
                    self.handle_command("/sessions", ReplOutput::Tui(&tui_state))
                        .await?;
                }
                TuiCmd::ResumeSession(session_id) => {
                    let current = self.runtime.session_id().to_string();
                    if session_id == current {
                        if let Ok(mut g) = tui_state.lock() {
                            g.blocks
                                .push(DisplayBlock::System("Already on this session.".into()));
                        }
                    } else {
                        let _ = self.runtime.save().await;
                        if let Ok(mut g) = tui_state.lock() {
                            g.blocks.push(DisplayBlock::System(format!(
                                "Resuming session {} is not yet fully supported in-process. Please restart nca with: nca resume {session_id}",
                                session_id
                            )));
                        }
                    }
                }
                TuiCmd::CycleModel(forward) => {
                    let recent = &self.runtime.config().model.recent_models;
                    if recent.len() >= 2 {
                        let current = self.runtime.model().to_string();
                        let pos = recent.iter().position(|m| m == &current).unwrap_or(0);
                        let next_pos = if forward {
                            (pos + 1) % recent.len()
                        } else {
                            pos.checked_sub(1).unwrap_or(recent.len() - 1)
                        };
                        let next_model = recent[next_pos].clone();
                        let mut cfg = self.runtime.config().clone();
                        cfg.apply_model_override(&next_model);
                        if let Ok(()) = self.runtime.apply_nca_config(cfg) {
                            let _ = self
                                .runtime
                                .config()
                                .save_workspace_file(self.runtime.workspace_root());
                            if let Ok(mut g) = tui_state.lock() {
                                g.model = self.runtime.model().to_string();
                                g.blocks.push(DisplayBlock::System(format!(
                                    "[F2] switched to {}",
                                    self.runtime.model()
                                )));
                            }
                        }
                    } else if let Ok(mut g) = tui_state.lock() {
                        g.blocks.push(DisplayBlock::System(
                            "[F2] no recent models to cycle (need 2+ in model.recent_models)"
                                .into(),
                        ));
                    }
                }
                TuiCmd::ValidateApiKey(provider, api_key) => {
                    if let Ok(mut g) = tui_state.lock() {
                        g.validation_status =
                            Some(crate::tui::state::OnboardingValidation::Validating);
                    }
                    let base_url = self
                        .runtime
                        .config()
                        .provider
                        .base_url_for(provider)
                        .to_string();
                    let result = nca_core::provider::validate::validate_api_key(
                        provider,
                        &api_key,
                        &base_url,
                        (provider == ProviderKind::Custom)
                            .then_some(self.runtime.config().provider.custom.compatibility),
                    )
                    .await;
                    if let Ok(mut g) = tui_state.lock() {
                        match &result {
                            nca_core::provider::validate::ValidationResult::Valid => {
                                g.validation_status =
                                    Some(crate::tui::state::OnboardingValidation::Valid);
                                g.close_api_key_modal();
                                g.close_connect_modal();
                                g.onboarding_mode = false;
                            }
                            nca_core::provider::validate::ValidationResult::InvalidKey(msg) => {
                                g.validation_status = Some(
                                    crate::tui::state::OnboardingValidation::Failed(msg.clone()),
                                );
                            }
                            nca_core::provider::validate::ValidationResult::NetworkError(msg) => {
                                g.validation_status = Some(
                                    crate::tui::state::OnboardingValidation::Failed(msg.clone()),
                                );
                            }
                        }
                    }
                    if matches!(
                        result,
                        nca_core::provider::validate::ValidationResult::Valid
                    ) {
                        let mut cfg = self.runtime.config().clone();
                        cfg.set_provider_api_key(provider, &api_key);
                        cfg.set_default_provider(provider);
                        if let Err(e) = self.runtime.apply_nca_config(cfg) {
                            tracing::warn!("onboarding: provider apply failed: {e}");
                            if let Ok(mut g) = tui_state.lock() {
                                g.validation_status =
                                    Some(crate::tui::state::OnboardingValidation::Failed(format!(
                                        "Failed to apply provider: {e}"
                                    )));
                                g.onboarding_mode = true;
                            }
                            continue;
                        }
                        if let Ok(mut g) = tui_state.lock() {
                            g.model = self.runtime.model().to_string();
                        }
                        let mut cfg = self.runtime.config().clone();
                        cfg.ui.onboarding_completed = true;
                        if let Err(e) = cfg.save_global() {
                            tracing::warn!("onboarding: global config save failed: {e}");
                        }
                        let _ = self.runtime.apply_nca_config(cfg);
                    }
                }
                TuiCmd::CompleteOnboarding => {
                    let mut cfg = self.runtime.config().clone();
                    cfg.ui.onboarding_completed = true;
                    if let Err(e) = cfg.save_global() {
                        tracing::warn!("onboarding flag save failed: {e}");
                    }
                    if let Err(e) = self.runtime.apply_nca_config(cfg) {
                        tracing::warn!("onboarding config apply failed: {e}");
                    }
                }
                TuiCmd::QuestionAnswer(selection) => {
                    let qid = if let Ok(g) = tui_state.lock() {
                        g.active_question.as_ref().map(|q| q.question_id.clone())
                    } else {
                        None
                    };
                    if let Some(qid) = qid
                        && !self.runtime.submit_question_answer(&qid, selection)
                        && let Ok(mut g) = tui_state.lock()
                    {
                        g.push_error(
                            "failed to submit answer (expired or already answered)".into(),
                        );
                    }
                }
                TuiCmd::Submit(line) => {
                    let line = line.trim().to_string();
                    let api_key_modal_state = tui_state.lock().ok().and_then(|g| {
                        g.api_key_modal_open().then_some((
                            g.api_key_target_provider(),
                            g.api_key_input().to_string(),
                            g.api_key_connect_after_save(),
                        ))
                    });
                    if let Some((Some(p), key_input, connect_after_save)) = api_key_modal_state {
                        let typed = if line.starts_with('/') {
                            ""
                        } else {
                            key_input.trim()
                        };
                        let had_existing = self.runtime.config().provider.api_key_present_for(p);
                        if line.starts_with('/') {
                            if let Ok(mut g) = tui_state.lock() {
                                g.close_api_key_modal();
                            }
                        } else if typed.is_empty() {
                            if had_existing {
                                if let Ok(mut g) = tui_state.lock() {
                                    g.close_api_key_modal();
                                    g.blocks.push(DisplayBlock::System(format!(
                                        "[apikey] keeping existing key for {}",
                                        p.display_name()
                                    )));
                                }
                                if connect_after_save {
                                    self.apply_provider_in_session(p, ReplOutput::Tui(&tui_state))
                                        .await?;
                                }
                            } else if let Ok(mut g) = tui_state.lock() {
                                g.push_error(format!(
                                    "[apikey] paste a key for {} or Esc to cancel",
                                    p.display_name()
                                ));
                            }
                            continue;
                        } else {
                            self.save_provider_api_key(p, typed, ReplOutput::Tui(&tui_state))
                                .await?;
                            if let Ok(mut g) = tui_state.lock() {
                                g.close_api_key_modal();
                            }
                            if connect_after_save {
                                self.apply_provider_in_session(p, ReplOutput::Tui(&tui_state))
                                    .await?;
                            }
                            continue;
                        }
                    }
                    if line.is_empty() {
                        if let Ok(mut g) = tui_state.lock()
                            && g.pending_api_key_provider.take().is_some()
                        {
                            g.blocks.push(DisplayBlock::System(
                                "[apikey] entry cancelled (empty line)".into(),
                            ));
                        }
                        continue;
                    }
                    if let Some(p) = tui_state
                        .lock()
                        .ok()
                        .and_then(|g| g.pending_api_key_provider)
                    {
                        if !line.starts_with('/') {
                            let mut cfg = self.runtime.config().clone();
                            cfg.set_provider_api_key(p, &line);
                            match self.runtime.apply_nca_config(cfg) {
                                Ok(()) => {
                                    if let Err(e) = self
                                        .runtime
                                        .config()
                                        .save_workspace_file(self.runtime.workspace_root())
                                    {
                                        if let Ok(mut g) = tui_state.lock() {
                                            g.push_error(format!(
                                                "[apikey] applied but save failed: {e}"
                                            ));
                                        }
                                    } else if let Ok(mut g) = tui_state.lock() {
                                        g.pending_api_key_provider = None;
                                        g.blocks.push(DisplayBlock::System(format!(
                                            "[apikey] saved for {}",
                                            p.display_name()
                                        )));
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut g) = tui_state.lock() {
                                        g.push_error(format!("[apikey] {e}"));
                                    }
                                }
                            }
                            continue;
                        }
                        if let Ok(mut g) = tui_state.lock() {
                            g.pending_api_key_provider = None;
                        }
                    }
                    if line.starts_with('!') {
                        let shell_cmd = line.trim_start_matches('!').trim();
                        self.run_bash_tui(shell_cmd, &tui_state).await;
                        continue;
                    }
                    if line.starts_with('/') {
                        if !self
                            .handle_command(&line, ReplOutput::Tui(&tui_state))
                            .await?
                        {
                            if let Ok(mut g) = tui_state.lock() {
                                g.should_exit = true;
                            }
                            break;
                        }
                        continue;
                    }
                    let expanded =
                        match expand_at_file_mentions_default(&line, self.runtime.workspace_root())
                        {
                            Ok(s) => s,
                            Err(e) => {
                                if let Ok(mut g) = tui_state.lock() {
                                    g.push_error(format!("file mentions: {e}"));
                                }
                                continue;
                            }
                        };
                    if let Ok(mut g) = tui_state.lock() {
                        g.set_busy(true);
                    }
                    let attachments = if let Ok(mut g) = tui_state.lock() {
                        std::mem::take(&mut g.staged_image_attachments)
                    } else {
                        Vec::new()
                    };
                    let turn = if attachments.is_empty() {
                        self.runtime.run_turn(&expanded).await
                    } else {
                        self.runtime
                            .run_turn_with_images(&expanded, attachments)
                            .await
                    };
                    if let Err(e) = turn
                        && let Ok(mut g) = tui_state.lock()
                    {
                        g.push_error(e.to_string());
                    }
                    if let Ok(mut g) = tui_state.lock() {
                        g.set_busy(false);
                    }
                }
            }
        }

        let _ = ui.await;
        self.runtime.finish(EndReason::UserExit).await;
        Ok(())
    }

    pub(super) async fn run_bash_tui(&self, cmd: &str, st: &Arc<Mutex<TuiSessionState>>) {
        fn log(st: &Arc<Mutex<TuiSessionState>>, s: &str) {
            if let Ok(mut g) = st.lock() {
                g.blocks.push(DisplayBlock::System(s.to_string()));
            }
        }
        if cmd.is_empty() {
            log(st, "! usage: !<command>");
            return;
        }
        log(st, &format!("[bash] {cmd}"));
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stdout.is_empty()
                    && let Ok(mut g) = st.lock()
                {
                    for line in stdout.lines() {
                        g.blocks.push(DisplayBlock::System(line.to_string()));
                    }
                }
                if !stderr.is_empty() {
                    log(st, &format!("[stderr] {stderr}"));
                }
                log(
                    st,
                    &if out.status.success() {
                        "[bash] exit 0".into()
                    } else {
                        format!("[bash] exit {}", out.status.code().unwrap_or(-1))
                    },
                );
            }
            Err(e) => log(st, &format!("[bash] {e}")),
        }
    }
}
