//! Status bar component — renders model, branch, tokens, cost, busy state, etc.

use std::time::Instant;

use nca_common::event::BusyState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::busy_indicator;

/// Theme colors used in the status bar (from shared module, re-exported via searchable_list).
use super::searchable_list::theme;

/// Data needed to render the status bar.
///
/// This is a snapshot of the relevant fields from `TuiSessionState`.
/// In Phase 3, NcaModel will populate this from TuiFeedbackMsg updates.
#[derive(Debug, Clone)]
pub(crate) struct StatusBarData {
    pub model: String,
    pub agent_profile: String,
    pub current_branch: String,
    pub permission_mode: String,
    pub session_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub context_usage_percent: u8,
    pub current_busy_state: BusyState,
    pub busy_state_since: Instant,
    pub active_approval: bool,
    pub active_question: bool,
    pub started: Instant,
}

impl Default for StatusBarData {
    fn default() -> Self {
        Self {
            model: String::from("unknown"),
            agent_profile: String::from("Build"),
            current_branch: String::new(),
            permission_mode: String::from("ask"),
            session_id: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            context_usage_percent: 0,
            current_busy_state: BusyState::Idle,
            busy_state_since: Instant::now(),
            active_approval: false,
            active_question: false,
            started: Instant::now(),
        }
    }
}

/// Status bar component.
///
/// Renders the bottom bar showing model, branch, tokens, cost, busy state, etc.
/// Ported from the inline rendering in `run_blocking()`.
pub(crate) struct StatusBar {
    data: StatusBarData,
    /// Whether sidebar is visible (affects whether tokens/cost show on bar).
    pub(crate) sidebar_visible: bool,
}

impl StatusBar {
    pub(crate) fn new() -> Self {
        Self {
            data: StatusBarData::default(),
            sidebar_visible: true,
        }
    }

    /// Update the status bar data snapshot.
    pub(crate) fn update_data(&mut self, data: StatusBarData) {
        self.data = data;
    }

    // ── Individual setters for NcaModel feedback routing ──

    pub(crate) fn update_session(&mut self, session_id: &str, model: &str) {
        self.data.session_id = session_id.to_string();
        self.data.model = model.to_string();
    }

    pub(crate) fn update_model(&mut self, model: &str) {
        self.data.model = model.to_string();
    }

    pub(crate) fn update_agent_profile(&mut self, label: &str) {
        self.data.agent_profile = label.to_string();
    }

    pub(crate) fn update_permission_mode(&mut self, mode: &str) {
        self.data.permission_mode = mode.to_string();
    }

    pub(crate) fn update_branch(&mut self, branch: &str) {
        self.data.current_branch = branch.to_string();
    }

    pub(crate) fn set_busy(&mut self, state: BusyState) {
        self.data.current_busy_state = state;
        if state == BusyState::Idle {
            self.data.busy_state_since = std::time::Instant::now();
        }
    }

    pub(crate) fn update_cost(&mut self, input: u64, output: u64, cost: f64) {
        self.data.input_tokens = input;
        self.data.output_tokens = output;
        self.data.cost_usd = cost;
    }

    pub(crate) fn update_context(&mut self, window: usize, usage: usize) {
        let _ = window;
        self.data.context_usage_percent = usage.clamp(0, 100) as u8;
    }

    pub(crate) fn set_active_approval(&mut self, active: bool) {
        self.data.active_approval = active;
    }

    pub(crate) fn set_active_question(&mut self, active: bool) {
        self.data.active_question = active;
    }

    /// Render the status bar into the given area.
    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let d = &self.data;

        // Busy indicator
        let indicator_text =
            busy_indicator::render_indicator(d.current_busy_state, d.busy_state_since);
        let indicator_color = busy_indicator::color_for_state(d.current_busy_state);
        let busy = Span::styled(indicator_text, Style::default().fg(indicator_color));

        // Approval hint
        let approval_hint = if d.active_approval {
            Span::styled(" !approve ", Style::default().fg(theme::ERROR))
        } else {
            Span::raw("")
        };

        // Question hint
        let q_hint = if d.active_question {
            Span::styled(" ?answer ", Style::default().fg(theme::WARN))
        } else {
            Span::raw("")
        };

        // Permission mode
        let perm_span = if toolbar_permission_is_bypass(&d.permission_mode) {
            Span::styled(
                " BYPASS — tools run without approval ",
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::ERROR)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" perm:{} ", d.permission_mode),
                Style::default().fg(theme::MUTED),
            )
        };

        // Timer
        let elapsed = d.started.elapsed().as_secs();
        let time_span = Span::styled(
            format!("{:02}:{:02}", elapsed / 60, elapsed % 60),
            Style::default().fg(theme::MUTED),
        );

        // Cancel hint (when busy, Esc cancels)
        let cancel_hint_text = " Esc cancel ";
        let cancel_visible = matches!(
            d.current_busy_state,
            BusyState::Thinking | BusyState::Streaming | BusyState::ToolRunning
        );
        let cancel_hint = cancel_visible.then(|| {
            Span::styled(
                cancel_hint_text,
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::WARN)
                    .add_modifier(Modifier::BOLD),
            )
        });

        // Layout: main status content (left) + cancel hint (right)
        let status_rect = if cancel_hint.is_some() && area.width > cancel_hint_text.len() as u16 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Length(cancel_hint_text.len() as u16),
                ])
                .split(area)[0]
        } else {
            area
        };

        // Branch chip
        let branch_text = if d.current_branch.is_empty() {
            String::new()
        } else {
            format!("⎇ {}", d.current_branch)
        };
        let branch_span_style = Style::default()
            .fg(theme::TOOL)
            .add_modifier(Modifier::UNDERLINED);

        // Build status spans
        let mut status_spans = vec![
            busy,
            approval_hint,
            q_hint,
            Span::raw(" │ "),
            Span::styled(&d.model, Style::default().fg(theme::USER)),
            Span::raw(" │ "),
            Span::styled(&d.agent_profile, Style::default().fg(theme::ASSISTANT)),
            Span::raw(" │ "),
            Span::styled(branch_text, branch_span_style),
            Span::raw(" │ "),
            perm_span,
        ];

        // Tokens/cost/session: show on bar only when sidebar is hidden
        if !self.sidebar_visible {
            status_spans.push(Span::raw(" │ "));
            status_spans.push(Span::styled(
                d.session_id[..8.min(d.session_id.len())].to_string(),
                Style::default().fg(theme::MUTED),
            ));
            status_spans.extend([
                Span::raw(" │ in:"),
                Span::styled(
                    format!("{}", d.input_tokens),
                    Style::default().fg(theme::TEXT),
                ),
                Span::raw(" out:"),
                Span::styled(
                    format!("{}", d.output_tokens),
                    Style::default().fg(theme::TEXT),
                ),
                Span::raw(" │ $"),
                Span::styled(
                    format!("{:.4}", d.cost_usd),
                    Style::default().fg(theme::SUCCESS),
                ),
            ]);
        }

        // Context usage percentage (always shown)
        let ctx_pct_color = if d.context_usage_percent >= 90 {
            theme::ERROR
        } else if d.context_usage_percent >= 70 {
            theme::WARN
        } else {
            theme::MUTED
        };
        status_spans.push(Span::raw(" │ ctx:"));
        status_spans.push(Span::styled(
            format!("{}%", d.context_usage_percent),
            Style::default().fg(ctx_pct_color),
        ));
        status_spans.push(Span::raw(" │ "));
        status_spans.push(time_span);

        // Render main bar
        let status = Line::from(status_spans);
        let bar = Paragraph::new(status).style(Style::default().bg(theme::SURFACE));
        frame.render_widget(bar, status_rect);

        // Render cancel hint overlay (right-aligned)
        if let Some(cancel_hint) = cancel_hint {
            let hint_width = cancel_hint_text.len() as u16;
            if area.width > hint_width {
                let hint_rect = Rect::new(
                    area.x + area.width.saturating_sub(hint_width),
                    area.y,
                    hint_width,
                    1,
                );
                let hint_bar = Paragraph::new(Line::from(cancel_hint))
                    .style(Style::default().bg(theme::SURFACE));
                frame.render_widget(hint_bar, hint_rect);
            }
        }
    }
}

fn toolbar_permission_is_bypass(mode: &str) -> bool {
    mode.contains("BypassPermissions")
}
