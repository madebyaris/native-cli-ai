//! # nca-autoresearch
//!
//! Autonomous research loop for nca — enables AI agents to run experiments
//! iteratively with fixed time budgets, measuring metrics and autonomously
//! deciding to keep or discard changes.
//!
//! ## Core Concepts
//!
//! - **Research Program**: Defines the editable files, metric extraction,
//!   time budget, and constraints for autonomous research.
//! - **Experiment**: A single run within the research loop.
//! - **Results Log**: TSV file tracking all experiment results.
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use nca_autoresearch::{ResearchProgram, ExperimentRunner};
//!
//! let program = ResearchProgram::from_file("my-research.md")?;
//! let runner = ExperimentRunner::new(program);
//!
//! let result = runner.run_experiment().await?;
//! println!("val_bpb: {}", result.metric_value);
//! ```

#![allow(clippy::pedantic)]

pub mod experiment;
pub mod git_integration;
pub mod loop_runner;
pub mod metric_parser;
pub mod program;
pub mod result;

pub use experiment::{ExperimentConfig, ExperimentRunner};
pub use git_integration::GitManager;
pub use loop_runner::AutoResearchLoop;
pub use metric_parser::MetricParser;
pub use program::{EditableFile, FixedFile, MetricCommand, ResearchProgram};
pub use result::{ExperimentResult, ExperimentStatus};

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

/// Auto-research session state
pub struct AutoResearchSession {
    pub program: ResearchProgram,
    pub results_path: PathBuf,
    pub worktree_path: PathBuf,
    pub current_branch: String,
    pub baseline_metric: Option<f64>,
    pub best_metric: f64,
    pub experiment_count: u64,
    pub session_id: String,

    /// Internal state protected by RwLock for async access
    inner: Arc<RwLock<SessionInner>>,
}

struct SessionInner {
    running: bool,
    should_stop: bool,
}

impl AutoResearchSession {
    /// Create a new auto-research session
    pub fn new(program: ResearchProgram, workspace: PathBuf, session_id: String) -> Self {
        let results_path = workspace.join("results.tsv");
        let worktree_path = workspace.join("experiments");

        Self {
            program,
            results_path,
            worktree_path,
            current_branch: format!("autoresearch/{}", session_id),
            baseline_metric: None,
            best_metric: f64::INFINITY,
            experiment_count: 0,
            session_id,
            inner: Arc::new(RwLock::new(SessionInner {
                running: false,
                should_stop: false,
            })),
        }
    }

    /// Check if the session is currently running
    pub fn is_running(&self) -> bool {
        self.inner.read().running
    }

    /// Start the session
    pub fn start(&self) {
        self.inner.write().running = true;
        self.inner.write().should_stop = false;
    }

    /// Stop the session
    pub fn stop(&self) {
        self.inner.write().should_stop = true;
    }

    /// Check if should stop
    pub fn should_stop(&self) -> bool {
        self.inner.read().should_stop
    }

    /// Record an experiment result
    pub fn record_result(&mut self, result: &ExperimentResult) {
        if result.status == ExperimentStatus::Keep {
            if result.metric_value < self.best_metric {
                self.best_metric = result.metric_value;
            }
            if self.baseline_metric.is_none() {
                self.baseline_metric = Some(result.metric_value);
            }
        }
        self.experiment_count += 1;
    }

    /// Check if metric improved compared to baseline
    pub fn metric_improved(&self, value: f64) -> bool {
        match self.baseline_metric {
            Some(baseline) => value < baseline,
            None => true, // First experiment is always "improvement"
        }
    }

    /// Get summary statistics
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id.clone(),
            program_name: self.program.name.clone(),
            experiment_count: self.experiment_count,
            baseline_metric: self.baseline_metric,
            best_metric: self.best_metric,
            current_branch: self.current_branch.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub program_name: String,
    pub experiment_count: u64,
    pub baseline_metric: Option<f64>,
    pub best_metric: f64,
    pub current_branch: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_lifecycle() {
        use program::{EditableFile, FixedFile, MetricCommand, MetricGoal};

        let program = ResearchProgram {
            name: "test".to_string(),
            description: "Test program".to_string(),
            editable_files: vec![EditableFile::new("train.py")],
            fixed_files: vec![FixedFile {
                path: std::path::PathBuf::from("prepare.py"),
                reason: "Fixed".to_string(),
            }],
            metric_command: MetricCommand {
                command: "grep val_bpb".to_string(),
                parse_regex: r"val_bpb:\s*([\d.]+)".to_string(),
            },
            metric_goal: MetricGoal::Minimize,
            time_budget_seconds: 300,
            extra_constraints: vec![],
            max_memory_gb: Some(50.0),
            instructions: String::new(),
        };

        let session = AutoResearchSession::new(program, PathBuf::from("."), "test".into());

        assert!(!session.is_running());
        session.start();
        assert!(session.is_running());
        session.stop();
        assert!(session.should_stop());
    }

    #[test]
    fn test_metric_improvement() {
        use program::{MetricCommand, MetricGoal};

        let program = ResearchProgram {
            name: "test".to_string(),
            description: "Test".to_string(),
            editable_files: vec![],
            fixed_files: vec![],
            metric_command: MetricCommand {
                command: "echo".to_string(),
                parse_regex: r"([\d.]+)".to_string(),
            },
            metric_goal: MetricGoal::Minimize,
            time_budget_seconds: 300,
            extra_constraints: vec![],
            max_memory_gb: None,
            instructions: String::new(),
        };

        let session = AutoResearchSession::new(program, PathBuf::from("."), "test".into());

        // First experiment always improves
        assert!(session.metric_improved(1.0));

        let result = ExperimentResult {
            commit: "abc".to_string(),
            metric_value: 0.9,
            memory_gb: 10.0,
            training_seconds: 300.0,
            total_seconds: 310.0,
            status: ExperimentStatus::Keep,
            description: "improved".to_string(),
            timestamp: chrono::Utc::now(),
            peak_vram_mb: None,
            mfu_percent: None,
            total_tokens_m: None,
            num_steps: None,
            num_params_m: None,
        };

        let mut session = session;
        session.record_result(&result);

        // Should now have baseline
        assert!(session.baseline_metric.is_some());
        assert!(!session.metric_improved(0.95)); // Worse than 0.9
        assert!(session.metric_improved(0.85)); // Better than 0.9
    }
}
