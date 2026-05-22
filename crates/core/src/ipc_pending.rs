//! Type aliases for IPC pending maps (keeps clippy::type_complexity happy).
//!
//! Lives in `nca-core` so that CLI and TUI layers can share the same
//! approval/question oneshot pending-map types without creating a dependency
//! on the CLI crate.

use crate::approval::ApprovalVerdict;
use nca_common::event::QuestionSelection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type ApprovalPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalVerdict>>>>;
pub type QuestionPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>;
