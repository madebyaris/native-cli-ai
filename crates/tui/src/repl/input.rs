//! REPL tab-completion: workspace `@`-mentions, slash commands, bash-mode
//! command hints, and skill discovery.
//!
//! Extracted from `repl/mod.rs` in Phase 2.3; depends on `Repl` for access to
//! the active session's workspace root and config.

use super::Repl;
use crate::file_mentions::{at_token_before_cursor, discover_workspace_files, filter_paths_prefix};
use crate::slash_commands::SLASH_COMMANDS;
use nca_core::skills::SkillCatalog;
use reedline::{Completer, Suggestion};

/// Tab completion for REPL commands and skills
impl Completer for Repl {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        if let Some((at_byte, prefix)) = at_token_before_cursor(line, pos) {
            let files = discover_workspace_files(self.runtime.workspace_root());
            for path in filter_paths_prefix(&files, &prefix) {
                suggestions.push(Suggestion {
                    value: format!("@{path}"),
                    description: Some("workspace file".to_string()),
                    extra: None,
                    span: reedline::Span {
                        start: at_byte,
                        end: pos,
                    },
                    append_whitespace: false,
                    style: None,
                });
            }
            if !suggestions.is_empty() {
                return suggestions;
            }
        }

        if line.starts_with('/') {
            for cmd in SLASH_COMMANDS {
                if cmd.starts_with(line) {
                    suggestions.push(Suggestion {
                        value: cmd.to_string(),
                        description: Some("REPL command".to_string()),
                        extra: None,
                        span: reedline::Span { start: 0, end: 0 },
                        append_whitespace: true,
                        style: None,
                    });
                }
            }
        }

        if line.starts_with('!') {
            let bash_commands = [
                "git", "ls", "cat", "find", "grep", "npm", "cargo", "make", "docker", "curl",
            ];
            let _prefix = line.trim_start_matches('!');
            for cmd in bash_commands {
                let full = format!("!{}", cmd);
                if full.starts_with(line) {
                    suggestions.push(Suggestion {
                        value: full,
                        description: Some("Shell command".to_string()),
                        extra: None,
                        span: reedline::Span { start: 0, end: 0 },
                        append_whitespace: true,
                        style: None,
                    });
                }
            }
        }

        if let Ok(skills) = SkillCatalog::discover(
            self.runtime.workspace_root(),
            &self.runtime.config().harness.skill_directories,
        ) {
            for skill in skills {
                let skill_cmd = format!("/{}", skill.command);
                if skill_cmd.starts_with(line) {
                    suggestions.push(Suggestion {
                        value: skill_cmd,
                        description: skill.description,
                        extra: None,
                        span: reedline::Span { start: 0, end: 0 },
                        append_whitespace: true,
                        style: None,
                    });
                }
            }
        }

        suggestions
    }
}
