//! Messages processed by the Elm update loop.

use crossterm::event::{KeyEvent, MouseEvent};
use nca_common::event::{AgentEvent, QuestionSelection};

use crate::tui::app::TuiCmd;

/// All messages that flow through the Elm architecture update loop.
#[derive(Debug)]
pub(crate) enum Msg {
    // ── Crossterm events ──────────────────────────────────────────
    /// Keyboard input event.
    Key(KeyEvent),
    /// Mouse input event.
    Mouse(MouseEvent),
    /// Pasted text from bracketed paste.
    Paste(String),
    /// Terminal was resized.
    Resize(u16, u16),

    // ── Agent events (from bridge via channel) ─────────────────────
    /// Raw agent event from the runtime, processed by update() into state changes.
    Agent(AgentEvent),

    // ── Internal TUI signals ─────────────────────────────────────
    /// Command from TUI to external runtime (submit, branch switch, etc.).
    Cmd(TuiCmd),
    /// Direct question answer — bypasses cmd_tx because run_turn is blocked.
    QuestionAnswer(QuestionSelection),
    /// Raw input for the active question — parsed and routed by the model.
    QuestionSubmit(String),
    /// Raw input for the active approval — parsed and routed by the model.
    ApprovalSubmit(String),
    /// Quick approval: approve (Ctrl+Y), deny (Enter with "n"), or always-allow (Ctrl+U).
    ApprovalQuickAnswer { approved: bool, always_allow: bool },
    /// Force a redraw (e.g. after busy indicator animation tick).
    Redraw,
    /// Request application exit.
    Quit,
}
