//! Global keyboard shortcut dispatcher.
//!
//! Intercepts key events that apply regardless of focus:
//! - Ctrl+Q: quit
//! - Ctrl+X leader key (M/E/L/N/C/S/A/H/Q)
//! - Ctrl+C: cancel turn
//! - Ctrl+L: clear screen
//! - Ctrl+P: open command palette
//! - Esc: cancel active turn
//! - F2/Shift+F2: cycle model

use crossterm::event::{KeyCode, KeyModifiers};

use super::super::msg::Msg;
use crate::tui::app::TuiCmd;

/// Whether the leader key (Ctrl+X) is pending.
#[derive(Debug, Default)]
pub(crate) struct GlobalListener {
    leader_pending: bool,
}

impl GlobalListener {
    pub(crate) fn new() -> Self {
        Self {
            leader_pending: false,
        }
    }

    /// Process a message. Returns `Some(Msg)` if a global shortcut was triggered.
    /// The caller should pass `Msg::Key(key)` events here before component dispatch.
    pub(crate) fn on(&mut self, msg: &Msg) -> Option<Msg> {
        let Msg::Key(key) = msg else {
            return None;
        };

        // Ctrl+X leader key dispatch
        if self.leader_pending {
            self.leader_pending = false;
            return match key.code {
                KeyCode::Char('m') | KeyCode::Char('M') => Some(Msg::Cmd(TuiCmd::OpenModelPicker)),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(Msg::Cmd(TuiCmd::OpenEditor)),
                KeyCode::Char('l') | KeyCode::Char('L') => Some(Msg::Cmd(TuiCmd::OpenSessions)),
                KeyCode::Char('n') | KeyCode::Char('N') => Some(Msg::Cmd(TuiCmd::NewSession)),
                KeyCode::Char('c') | KeyCode::Char('C') => Some(Msg::Cmd(TuiCmd::RunCompact)),
                KeyCode::Char('s') | KeyCode::Char('S') => Some(Msg::Cmd(TuiCmd::OpenStatus)),
                KeyCode::Char('a') | KeyCode::Char('A') => Some(Msg::Cmd(TuiCmd::OpenAgentPicker)),
                KeyCode::Char('h') | KeyCode::Char('H') => Some(Msg::Cmd(TuiCmd::OpenHelp)),
                KeyCode::Char('q') | KeyCode::Char('Q') => Some(Msg::Quit),
                _ => None,
            };
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => Some(Msg::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
                if !key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Some(Msg::Cmd(TuiCmd::CancelTurn))
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                // Ctrl+L: clear transcript. This will be handled by NcaModel in Phase 1c.
                // For now, emit a no-op Cmd that NcaModel can intercept.
                None // Will be wired in Phase 3
            }
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                // Ctrl+P: open command palette. Will be wired in Phase 2.
                None
            }
            (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                self.leader_pending = true;
                None
            }
            (KeyCode::Esc, KeyModifiers::NONE) => Some(Msg::Cmd(TuiCmd::CancelTurn)),
            (KeyCode::F(2), KeyModifiers::NONE) => Some(Msg::Cmd(TuiCmd::CycleModel(true))),
            (KeyCode::F(2), KeyModifiers::SHIFT) => Some(Msg::Cmd(TuiCmd::CycleModel(false))),
            _ => None,
        }
    }
}
