//! Sub-agent (child session) spawning.

use super::{AutoDenyHandler, Supervisor, SupervisorConfig, spawn_event_fanout};
use crate::session_store::SessionStore;
use nca_common::config::NcaConfig;
use nca_common::event::{AgentEvent, EndReason};
use nca_core::approval::ApprovalHandler;
use nca_core::hooks::{HookEventKind, HookRunner};
use nca_core::skills::SkillCatalog;
use nca_core::tools::spawn_subagent::SpawnRequest;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Configuration for spawning a child session.
pub struct ChildSessionConfig {
    pub parent_session_id: String,
    pub task: String,
    pub workspace_root: PathBuf,
    pub config: NcaConfig,
    pub parent_summary: String,
    pub use_worktree: bool,
    pub focus_files: Vec<String>,
    pub skills: Vec<String>,
}

/// Result of a spawned child session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChildSessionResult {
    pub child_session_id: String,
    pub status: String,
    pub output: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

/// Spawns a background task that consumes spawn requests from the sub-agent tool
/// and runs child sessions. Each child session inherits parent context.
pub fn spawn_subagent_consumer(
    mut spawn_rx: mpsc::Receiver<SpawnRequest>,
    parent_session_id: String,
    workspace_root: PathBuf,
    config: NcaConfig,
    parent_messages: Vec<nca_common::message::Message>,
    event_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let parent_sessions_dir = workspace_root.join(&config.session.history_dir);
        let parent_summary = build_parent_summary(&parent_messages);

        while let Some(req) = spawn_rx.recv().await {
            let parent_session_id = parent_session_id.clone();
            let workspace_root = workspace_root.clone();
            let config = config.clone();
            let event_tx = event_tx.clone();
            let parent_store = SessionStore::new(parent_sessions_dir.clone());
            let parent_summary = parent_summary.clone();
            let resolved_skills = match SkillCatalog::resolve_requested_commands(
                &workspace_root,
                &config.harness.skill_directories,
                &req.skills,
            ) {
                Ok(skills) => skills,
                Err(error) => {
                    if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(AgentEvent::Error {
                                message: format!("Failed to spawn child session: {error}"),
                            })
                            .await;
                    }
                    let response = nca_core::tools::spawn_subagent::SpawnResponse {
                        child_session_id: String::new(),
                        status: "error".into(),
                        output: error,
                        workspace: workspace_root.display().to_string(),
                        branch: None,
                        worktree_path: None,
                    };
                    let _ = req.reply.send(response);
                    continue;
                }
            };

            let child_cfg = ChildSessionConfig {
                parent_session_id: parent_session_id.clone(),
                task: req.task.clone(),
                workspace_root: workspace_root.clone(),
                config,
                parent_summary,
                use_worktree: req.use_worktree,
                focus_files: req.focus_files,
                skills: resolved_skills,
            };

            tokio::spawn(async move {
                let hook_runner = {
                    let runner = HookRunner::new(child_cfg.config.hooks.clone());
                    runner.has_any().then_some(runner)
                };
                if let Some(hooks) = &hook_runner {
                    hooks
                        .run_best_effort(
                            HookEventKind::SubagentStart,
                            None,
                            &json!({
                                "parent_session_id": parent_session_id.clone(),
                                "task": child_cfg.task.clone(),
                                "workspace": child_cfg.workspace_root.clone(),
                            }),
                        )
                        .await;
                }
                let result = spawn_child_session(child_cfg, event_tx.clone()).await;
                match result {
                    Ok(res) => {
                        append_child_to_parent(
                            &parent_store,
                            &parent_session_id,
                            &res.child_session_id,
                        )
                        .await;

                        if let Some(ref tx) = event_tx {
                            let _ = tx
                                .send(AgentEvent::ChildSessionCompleted {
                                    parent_session_id: parent_session_id.clone(),
                                    child_session_id: res.child_session_id.clone(),
                                    status: res.status.clone(),
                                })
                                .await;
                        }
                        if let Some(hooks) = &hook_runner {
                            hooks
                                .run_best_effort(
                                    HookEventKind::SubagentStop,
                                    None,
                                    &json!({
                                        "parent_session_id": parent_session_id.clone(),
                                        "child_session_id": res.child_session_id.clone(),
                                        "status": res.status.clone(),
                                    }),
                                )
                                .await;
                        }
                        let response = nca_core::tools::spawn_subagent::SpawnResponse {
                            child_session_id: res.child_session_id,
                            status: res.status,
                            output: res.output,
                            workspace: res.workspace,
                            branch: res.branch,
                            worktree_path: res.worktree_path,
                        };
                        let _ = req.reply.send(response);
                    }
                    Err(e) => {
                        if let Some(hooks) = &hook_runner {
                            hooks
                                .run_best_effort(
                                    HookEventKind::SubagentStop,
                                    None,
                                    &json!({
                                        "parent_session_id": parent_session_id.clone(),
                                        "status": "error",
                                        "error": e.clone(),
                                    }),
                                )
                                .await;
                        }
                        if let Some(ref tx) = event_tx {
                            let _ = tx
                                .send(AgentEvent::Error {
                                    message: format!("Failed to spawn child session: {e}"),
                                })
                                .await;
                        }
                        let response = nca_core::tools::spawn_subagent::SpawnResponse {
                            child_session_id: String::new(),
                            status: "error".into(),
                            output: e,
                            workspace: workspace_root.display().to_string(),
                            branch: None,
                            worktree_path: None,
                        };
                        let _ = req.reply.send(response);
                    }
                }
            });
        }
    })
}

/// Append a child session ID to the parent session's metadata on disk.
async fn append_child_to_parent(store: &SessionStore, parent_id: &str, child_id: &str) {
    if let Ok(mut parent) = store.load(parent_id).await
        && !parent
            .meta
            .child_session_ids
            .contains(&child_id.to_string())
    {
        parent.meta.child_session_ids.push(child_id.to_string());
        let _ = store.save(&parent).await;
    }
}

/// Build a concise summary of the parent conversation for context inheritance.
pub(super) fn build_parent_summary(messages: &[nca_common::message::Message]) -> String {
    use nca_common::message::Role;

    let mut summary = String::new();
    let recent: Vec<_> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant | Role::System))
        .collect();

    let window = if recent.len() > 10 {
        &recent[recent.len() - 10..]
    } else {
        &recent
    };

    for msg in window {
        let role = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => continue,
        };
        let body = msg.content.event_preview();
        let content = if body.len() > 500 {
            let truncated: String = body.chars().take(500).collect();
            format!("{truncated}...")
        } else {
            body
        };
        summary.push_str(&format!("[{role}]: {content}\n\n"));
    }

    summary
}

fn build_subagent_context_prompt(
    parent_summary: &str,
    task: &str,
    focus_files: &[String],
    skills: &[String],
) -> String {
    let mut context_prompt = format!(
        "You are a sub-agent spawned by a parent session to handle a specific task.\n\n\
         ## Parent Context\n{}\n\n\
         ## Your Task\n{}",
        parent_summary, task
    );

    if !skills.is_empty() {
        context_prompt.push_str(
            "\n\n## Recommended Skills\nLoad these with invoke_skill before starting if they apply:\n",
        );
        for skill in skills {
            context_prompt.push_str(&format!("- {skill}\n"));
        }
    }

    if !focus_files.is_empty() {
        context_prompt.push_str("\n\n## Focus Files\n");
        for file in focus_files {
            context_prompt.push_str(&format!("- {file}\n"));
        }
    }

    context_prompt
}

/// Spawn a child session that inherits parent context and runs to completion.
/// Returns the result of the child run. This is a blocking async call.
pub async fn spawn_child_session(
    cfg: ChildSessionConfig,
    event_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<ChildSessionResult, String> {
    // Child sessions are non-interactive and already authorized by the parent
    // approval. Elevate to BypassPermissions so sub-agents can write files,
    // run tools, and spawn their own children without being auto-denied.
    let mut child_config = cfg.config.clone();
    child_config.permissions.mode = nca_common::config::PermissionMode::BypassPermissions;

    let mut sup = Supervisor::create(SupervisorConfig {
        config: child_config,
        workspace_root: cfg.workspace_root.clone(),
        safe_mode: false,
        interactive_approvals: false,
        session_id: None,
        approval_handler: Some(Arc::new(AutoDenyHandler) as Arc<dyn ApprovalHandler>),
        approval_pending: None,
        orchestration_context: None,
        preloaded_state: None,
    })
    .await
    .map_err(|e| e.to_string())?;

    let child_id = sup.session_id.clone();

    sup.set_parent(
        cfg.parent_session_id.clone(),
        Some(cfg.parent_summary.clone()),
        Some(cfg.task.clone()),
    );

    if cfg.use_worktree {
        let wt_mgr = crate::worktree::WorktreeManager::new(&cfg.workspace_root);
        if wt_mgr.is_git_repo() {
            let child_id_for_wt = child_id.clone();
            let workspace_for_wt = cfg.workspace_root.clone();
            match tokio::task::spawn_blocking(move || {
                let mgr = crate::worktree::WorktreeManager::new(&workspace_for_wt);
                mgr.create_worktree(&child_id_for_wt)
            })
            .await
            {
                Ok(Ok(info)) => {
                    sup.set_worktree_info(
                        info.worktree_path.clone(),
                        info.branch_name.clone(),
                        info.base_branch.clone(),
                    );
                    sup.workspace_root = info.worktree_path;
                }
                Ok(Err(e)) => {
                    tracing::warn!("Failed to create worktree for child session: {e}");
                }
                Err(e) => {
                    tracing::warn!("Worktree task join failed for child session: {e}");
                }
            }
        }
    }

    if let Some(ref tx) = event_tx {
        let _ = tx
            .send(AgentEvent::ChildSessionSpawned {
                parent_session_id: cfg.parent_session_id.clone(),
                child_session_id: child_id.clone(),
                task: cfg.task.clone(),
                workspace: sup.workspace_root.clone(),
                branch: sup.branch.clone(),
            })
            .await;
    }

    let context_prompt = build_subagent_context_prompt(
        &cfg.parent_summary,
        &cfg.task,
        &cfg.focus_files,
        &cfg.skills,
    );

    let mut handle = sup.take_handle();
    let event_rx = handle.take_event_rx();
    let log_path = handle.event_log_path.clone();

    let parent_forward = event_tx.map(|tx| (child_id.clone(), tx));
    let fanout = event_rx.map(|rx| spawn_event_fanout(rx, log_path, None, None, parent_forward));

    let result = sup.run_turn(&context_prompt).await;

    let (status, output) = match result {
        Ok(text) => {
            sup.finish(EndReason::Completed).await;
            ("completed".to_string(), text)
        }
        Err(e) => {
            sup.finish(EndReason::Error).await;
            ("error".to_string(), e.to_string())
        }
    };

    if let Some(f) = fanout {
        f.abort();
    }

    let branch = sup.branch.clone();
    let wt_path = sup.worktree_path.clone().map(|p| p.display().to_string());

    Ok(ChildSessionResult {
        child_session_id: child_id,
        status,
        output,
        workspace: sup.workspace_root.display().to_string(),
        branch,
        worktree_path: wt_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn spawn_subagent_consumer_rejects_unknown_skill_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join(".nca/skills/review");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Review\ncommand: review\n---\nReview code.\n",
        )
        .expect("write skill");

        let (spawn_tx, spawn_rx) = mpsc::channel(1);
        let handle = spawn_subagent_consumer(
            spawn_rx,
            "parent-1".into(),
            temp.path().to_path_buf(),
            NcaConfig::default(),
            vec![],
            None,
        );

        let (reply_tx, reply_rx) = oneshot::channel();
        spawn_tx
            .send(SpawnRequest {
                task: "Review code".into(),
                focus_files: vec![],
                skills: vec!["unknown-skill".into()],
                use_worktree: false,
                reply: reply_tx,
            })
            .await
            .expect("send spawn request");

        let response = tokio::time::timeout(std::time::Duration::from_secs(1), reply_rx)
            .await
            .expect("reply timeout")
            .expect("reply channel");

        assert_eq!(response.status, "error");
        assert!(
            response
                .output
                .contains("Unknown skill name(s): unknown-skill")
        );
        assert!(response.output.contains("Available skills:"));
        assert!(response.output.contains("review"));

        handle.abort();
    }

    #[test]
    fn build_subagent_context_prompt_includes_skills_when_present() {
        let prompt = build_subagent_context_prompt(
            "Parent discussed parser cleanup.",
            "Refactor the parser.",
            &[String::from("src/parser.rs")],
            &[String::from("rust-refactor"), String::from("testing")],
        );

        assert!(prompt.contains("## Recommended Skills"));
        assert!(prompt.contains("Load these with invoke_skill before starting if they apply:"));
        assert!(prompt.contains("- rust-refactor"));
        assert!(prompt.contains("- testing"));
        assert!(prompt.contains("## Focus Files"));
    }

    #[test]
    fn build_subagent_context_prompt_omits_skills_section_when_empty() {
        let prompt = build_subagent_context_prompt(
            "Parent discussed parser cleanup.",
            "Refactor the parser.",
            &[],
            &[],
        );

        assert!(!prompt.contains("## Recommended Skills"));
        assert!(!prompt.contains("invoke_skill before starting"));
    }
}
