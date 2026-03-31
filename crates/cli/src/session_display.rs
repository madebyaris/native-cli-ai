use nca_common::session::SessionSnapshot;
use std::path::Path;

const SUMMARY_MAX: usize = 96;
const PICKER_SUMMARY_MAX: usize = 52;
const PATH_SUFFIX_MAX: usize = 36;

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn format_cost(snapshot: &SessionSnapshot) -> String {
    format!("${:.4}", snapshot.estimated_cost_usd)
}

fn format_path_suffix(path: &Path, max_chars: usize) -> String {
    let text = path.display().to_string();
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text;
    }
    let suffix: String = text
        .chars()
        .rev()
        .take(max_chars.saturating_sub(3))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("...{suffix}")
}

pub fn format_human_session_lines(snapshot: &SessionSnapshot) -> Vec<String> {
    let mut header = format!(
        "{}  status={:?}  model={}  updated={}  children={}",
        snapshot.id,
        snapshot.status,
        snapshot.model,
        snapshot.updated_at.to_rfc3339(),
        snapshot.child_session_ids.len()
    );
    if let Some(branch) = &snapshot.branch {
        header.push_str(&format!("  branch={branch}"));
    }
    if let Some(base) = &snapshot.base_branch {
        header.push_str(&format!("  base={base}"));
    }
    header.push_str(&format!(
        "  tokens={}/{}  cost={}",
        snapshot.total_input_tokens,
        snapshot.total_output_tokens,
        format_cost(snapshot)
    ));

    let mut lines = vec![header];
    if let Some(worktree) = &snapshot.worktree_path {
        lines.push(format!(
            "  worktree: {}",
            format_path_suffix(worktree, PATH_SUFFIX_MAX)
        ));
    }
    if let Some(summary) = &snapshot.session_summary {
        lines.push(format!(
            "  summary: {}",
            truncate(&summary.replace('\n', " "), SUMMARY_MAX)
        ));
    }
    lines
}

pub fn format_resume_briefing_lines(snapshot: &SessionSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("Resuming session {}", snapshot.id),
        format!(
            "Status: {:?}  Model: {}  Updated: {}",
            snapshot.status,
            snapshot.model,
            snapshot.updated_at.to_rfc3339()
        ),
    ];
    if snapshot.branch.is_some() || snapshot.base_branch.is_some() {
        lines.push(format!(
            "Branch: {}  Base: {}",
            snapshot.branch.as_deref().unwrap_or("-"),
            snapshot.base_branch.as_deref().unwrap_or("-")
        ));
    }
    if let Some(worktree) = &snapshot.worktree_path {
        lines.push(format!(
            "Worktree: {}",
            format_path_suffix(worktree, PATH_SUFFIX_MAX)
        ));
    }
    lines.push(format!(
        "Tokens: {}/{}  Cost: {}  Children: {}",
        snapshot.total_input_tokens,
        snapshot.total_output_tokens,
        format_cost(snapshot),
        snapshot.child_session_ids.len()
    ));
    if let Some(summary) = &snapshot.session_summary {
        lines.push(format!(
            "Summary: {}",
            truncate(&summary.replace('\n', " "), SUMMARY_MAX)
        ));
    }
    lines
}

pub fn format_session_picker_label(snapshot: &SessionSnapshot) -> String {
    let mut label = format!(
        "{}  [{:?}]  {}  updated={}",
        snapshot.id,
        snapshot.status,
        snapshot.model,
        snapshot.updated_at.format("%Y-%m-%d %H:%M")
    );
    if let Some(branch) = &snapshot.branch {
        label.push_str(&format!("  branch={branch}"));
    }
    label.push_str(&format!(
        "  cost={}  tokens={}/{}",
        format_cost(snapshot),
        snapshot.total_input_tokens,
        snapshot.total_output_tokens
    ));
    if let Some(summary) = &snapshot.session_summary {
        label.push_str(&format!(
            "  — {}",
            truncate(&summary.replace('\n', " "), PICKER_SUMMARY_MAX)
        ));
    }
    label
}

pub fn format_cost_lines(snapshot: &SessionSnapshot, event_log_path: &Path) -> Vec<String> {
    vec![
        format!("Session:     {}", snapshot.id),
        format!("Status:      {:?}", snapshot.status),
        format!("Model:       {}", snapshot.model),
        format!("Input:       {}", snapshot.total_input_tokens),
        format!("Output:      {}", snapshot.total_output_tokens),
        format!("Cost:        {}", format_cost(snapshot)),
        format!("Children:    {}", snapshot.child_session_ids.len()),
        format!("Event log:   {}", event_log_path.display()),
    ]
}

pub fn format_attach_lines(snapshot: &SessionSnapshot, event_log_path: &Path) -> Vec<String> {
    let mut lines = vec![
        format!("Session:     {}", snapshot.id),
        format!("Status:      {:?}", snapshot.status),
        format!(
            "Socket:      {}",
            snapshot
                .socket_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".into())
        ),
        format!("Event log:   {}", event_log_path.display()),
        format!("Attach cmd:  nca attach {}", snapshot.id),
        format!("Logs cmd:    nca logs {}", snapshot.id),
    ];
    if let Some(branch) = &snapshot.branch {
        lines.push(format!("Branch:      {branch}"));
    }
    lines
}

pub fn should_show_resume_briefing(snapshot: &SessionSnapshot, message_count: usize) -> bool {
    message_count > 1
        || snapshot.total_input_tokens > 0
        || snapshot.total_output_tokens > 0
        || !snapshot.child_session_ids.is_empty()
        || snapshot.session_summary.is_some()
        || snapshot.branch.is_some()
        || snapshot.worktree_path.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nca_common::session::SessionStatus;
    use std::path::PathBuf;

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            id: "session-123".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            workspace: PathBuf::from("/tmp/work"),
            model: "MiniMax-M2.5".into(),
            status: SessionStatus::Completed,
            pid: None,
            socket_path: Some(PathBuf::from("/tmp/nca/session-123.sock")),
            worktree_path: Some(PathBuf::from("/tmp/work/.nca/worktrees/session-123")),
            branch: Some("feature/demo".into()),
            base_branch: Some("main".into()),
            parent_session_id: None,
            child_session_ids: vec!["child-1".into()],
            inherited_summary: None,
            spawn_reason: None,
            session_summary: Some("Recovered startup context and staged a compact summary.".into()),
            orchestration: None,
            total_input_tokens: 123,
            total_output_tokens: 456,
            estimated_cost_usd: 0.0199,
        }
    }

    #[test]
    fn picker_label_includes_branch_cost_and_summary() {
        let label = format_session_picker_label(&snapshot());
        assert!(label.contains("feature/demo"));
        assert!(label.contains("$0.0199"));
        assert!(label.contains("Recovered startup context"));
    }

    #[test]
    fn resume_briefing_is_enabled_for_persisted_session_context() {
        assert!(should_show_resume_briefing(&snapshot(), 3));
    }

    #[test]
    fn cost_and_attach_lines_surface_machine_paths() {
        let snap = snapshot();
        let cost = format_cost_lines(
            &snap,
            Path::new("/tmp/work/.nca/sessions/session-123.events.jsonl"),
        );
        let attach = format_attach_lines(
            &snap,
            Path::new("/tmp/work/.nca/sessions/session-123.events.jsonl"),
        );
        assert!(cost.iter().any(|line| line.contains("123")));
        assert!(cost.iter().any(|line| line.contains("$0.0199")));
        assert!(
            attach
                .iter()
                .any(|line| line.contains("nca attach session-123"))
        );
        assert!(attach.iter().any(|line| line.contains("Event log:")));
    }
}
