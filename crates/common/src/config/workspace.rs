//! Workspace-scoped configuration: session storage and harness (skills/instructions) paths.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub history_dir: PathBuf,
    #[serde(alias = "max_turn_per_run")]
    pub max_turns_per_run: u32,
    pub max_tool_calls_per_turn: u32,
    pub checkpoint_interval: u32,
    /// File that stores the last active session ID for auto-resume.
    pub last_session_file: PathBuf,
    /// Auto-compact when switching away from a session.
    #[serde(default)]
    pub auto_compact_on_finish: bool,
    /// Stream `execute_bash` stdout/stderr as `ToolOutputChunk` events while
    /// the command runs. Default on; set to false to restore the single
    /// batch-at-completion behavior.
    #[serde(default = "default_stream_bash_output")]
    pub stream_bash_output: bool,
}

fn default_stream_bash_output() -> bool {
    true
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            history_dir: PathBuf::from(".nca/sessions"),
            max_turns_per_run: 128,
            max_tool_calls_per_turn: 200,
            checkpoint_interval: 5,
            last_session_file: PathBuf::from(".nca/.last_session"),
            auto_compact_on_finish: false,
            stream_bash_output: true,
        }
    }
}

impl SessionConfig {
    pub(super) fn merge(&mut self, partial: PartialSessionConfig) {
        if let Some(history_dir) = partial.history_dir {
            self.history_dir = history_dir;
        }
        if let Some(max_turns_per_run) = partial.max_turns_per_run {
            self.max_turns_per_run = max_turns_per_run;
        }
        if let Some(max_tool_calls_per_turn) = partial.max_tool_calls_per_turn {
            self.max_tool_calls_per_turn = max_tool_calls_per_turn;
        }
        if let Some(checkpoint_interval) = partial.checkpoint_interval {
            self.checkpoint_interval = checkpoint_interval;
        }
        if let Some(last_session_file) = partial.last_session_file {
            self.last_session_file = last_session_file;
        }
        if let Some(auto_compact_on_finish) = partial.auto_compact_on_finish {
            self.auto_compact_on_finish = auto_compact_on_finish;
        }
        if let Some(stream_bash_output) = partial.stream_bash_output {
            self.stream_bash_output = stream_bash_output;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub built_in_enabled: bool,
    pub project_instructions_path: PathBuf,
    pub local_instructions_path: PathBuf,
    pub skill_directories: Vec<PathBuf>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            built_in_enabled: true,
            project_instructions_path: PathBuf::from(".ncarc"),
            local_instructions_path: PathBuf::from(".nca/instructions.md"),
            skill_directories: default_skill_directories(),
        }
    }
}

impl HarnessConfig {
    pub(super) fn merge(&mut self, partial: PartialHarnessConfig) {
        if let Some(enabled) = partial.built_in_enabled {
            self.built_in_enabled = enabled;
        }
        if let Some(path) = partial.project_instructions_path {
            self.project_instructions_path = path;
        }
        if let Some(path) = partial.local_instructions_path {
            self.local_instructions_path = path;
        }
        if let Some(skill_directories) = partial.skill_directories {
            self.skill_directories = skill_directories;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialSessionConfig {
    pub(super) history_dir: Option<PathBuf>,
    #[serde(alias = "max_turn_per_run")]
    pub(super) max_turns_per_run: Option<u32>,
    pub(super) max_tool_calls_per_turn: Option<u32>,
    pub(super) checkpoint_interval: Option<u32>,
    pub(super) last_session_file: Option<PathBuf>,
    pub(super) auto_compact_on_finish: Option<bool>,
    pub(super) stream_bash_output: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialHarnessConfig {
    pub(super) built_in_enabled: Option<bool>,
    pub(super) project_instructions_path: Option<PathBuf>,
    pub(super) local_instructions_path: Option<PathBuf>,
    pub(super) skill_directories: Option<Vec<PathBuf>>,
}

fn default_skill_directories() -> Vec<PathBuf> {
    vec![
        PathBuf::from(".nca/skills"),
        PathBuf::from(".claude/skills"),
    ]
}
