//! Output sink for REPL / slash-command text.
//!
//! Abstracts over the two surfaces that slash commands and presets write to:
//! the plain stdout transcript (tty mode) and the full-screen `ratatui`
//! transcript (TUI mode). Keeping this in one place means slash handlers can
//! stay side-effect-free and oblivious to which surface they target.

use crate::tui::state::{DisplayBlock, TuiSessionState};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// Special input prefixes
#[allow(dead_code)]
pub(crate) const INPUT_PREFIXES: &[&str] = &[
    "!",  // Bash mode - run shell command directly
    "@",  // File reference - fuzzy file search
    "\\", // Multiline continuation
];

/// Where slash-command and preset output goes (TTY transcript vs full-screen TUI).
pub(crate) enum ReplOutput<'a> {
    Stdio,
    Tui(&'a Arc<Mutex<TuiSessionState>>),
}

impl ReplOutput<'_> {
    pub(crate) fn print(&self, s: &str) {
        match self {
            ReplOutput::Stdio => {
                print!("{s}");
                let _ = std::io::stdout().flush();
            }
            ReplOutput::Tui(st) => {
                if let Ok(mut g) = st.lock() {
                    for line in s.split('\n') {
                        g.blocks.push(DisplayBlock::System(line.to_string()));
                    }
                }
            }
        }
    }

    pub(crate) fn println(&self, s: &str) {
        self.print(&format!("{s}\n"));
    }

    pub(crate) fn eprintln(&self, s: &str) {
        match self {
            ReplOutput::Stdio => eprintln!("{s}"),
            ReplOutput::Tui(st) => {
                if let Ok(mut g) = st.lock() {
                    g.blocks.push(DisplayBlock::System(format!("[!] {s}")));
                }
            }
        }
    }

    pub(crate) fn clear_screen(&self) {
        match self {
            ReplOutput::Stdio => {
                print!("\x1B[2J\x1B[H");
                std::io::stdout().flush().ok();
            }
            ReplOutput::Tui(st) => {
                if let Ok(mut g) = st.lock() {
                    g.blocks.clear();
                    g.streaming_assistant = None;
                    g.scroll_lines = 0;
                }
            }
        }
    }
}
