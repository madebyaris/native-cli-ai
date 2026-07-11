//! Type aliases for IPC pending maps (keeps clippy::type_complexity happy).

use nca_common::event::QuestionSelection;
use nca_core::approval::ApprovalVerdict;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type ApprovalPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalVerdict>>>>;
pub type QuestionPendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>;
