//! Event streaming with beautiful TUI rendering
//!
//! This module provides streaming event rendering with Claude Code-inspired styling.

use colored::Colorize;
use nca_common::event::{AgentEvent, EventEnvelope, InteractiveQuestionPayload, QuestionSelection};
use nca_runtime::ipc::IpcHandle;
use nca_runtime::supervisor;
use nca_tui::ipc_pending::{ApprovalPendingMap, QuestionPendingMap};
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::oneshot;

type EventEnvelopeFn = Arc<dyn Fn(&EventEnvelope) + Send + Sync>;

/// Blocking stdin prompt for `--no-tui` human stream when `ask_question` runs.
fn prompt_question_stdio(q: &InteractiveQuestionPayload) -> io::Result<QuestionSelection> {
    let mut err = io::stderr();
    writeln!(err)?;
    writeln!(err, "[question] {}", q.prompt)?;
    writeln!(err, "  [0] suggested: {}", q.suggested_answer)?;
    for (i, o) in q.options.iter().enumerate() {
        writeln!(err, "  [{}] ({}) {}", i + 1, o.id, o.label)?;
    }
    if q.allow_custom {
        writeln!(err, "  [c] custom text")?;
    }
    write!(
        err,
        "Choice (0, 1–{}{}): ",
        q.options.len(),
        if q.allow_custom { ", c" } else { "" }
    )?;
    err.flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let t = line.trim();
    if t == "0" || t.eq_ignore_ascii_case("s") {
        return Ok(QuestionSelection::Suggested);
    }
    if q.allow_custom && (t.eq_ignore_ascii_case("c") || t.eq_ignore_ascii_case("custom")) {
        write!(err, "Custom answer> ")?;
        err.flush()?;
        let mut custom = String::new();
        io::stdin().read_line(&mut custom)?;
        let custom = custom.trim().to_string();
        if custom.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty custom answer",
            ));
        }
        return Ok(QuestionSelection::Custom { text: custom });
    }
    if let Ok(n) = t.parse::<usize>()
        && n >= 1
        && n <= q.options.len()
    {
        return Ok(QuestionSelection::Option {
            option_id: q.options[n - 1].id.clone(),
        });
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid choice; use 0, 1–n, or c",
    ))
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StreamMode {
    Off,
    Human,
    Ndjson,
}

/// Real-time streaming stats
#[derive(Clone)]
struct StreamStats {
    input_tokens: Arc<AtomicU64>,
    output_tokens: Arc<AtomicU64>,
    estimated_cost: Arc<AtomicU64>,
    #[allow(dead_code)]
    start_time: Instant,
}

impl StreamStats {
    fn new() -> Self {
        Self {
            input_tokens: Arc::new(AtomicU64::new(0)),
            output_tokens: Arc::new(AtomicU64::new(0)),
            estimated_cost: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
        }
    }

    fn update_cost(&self, input: u64, output: u64, cost_cents: u64) {
        self.input_tokens.store(input, Ordering::Relaxed);
        self.output_tokens.store(output, Ordering::Relaxed);
        self.estimated_cost.store(cost_cents, Ordering::Relaxed);
    }

    fn record_output_token(&self) {
        self.output_tokens.fetch_add(1, Ordering::Relaxed);
    }

    fn input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    fn output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    fn estimated_cost_usd(&self) -> f64 {
        self.estimated_cost.load(Ordering::Relaxed) as f64 / 100.0
    }

    #[allow(dead_code)]
    fn elapsed_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}

impl Default for StreamStats {
    fn default() -> Self {
        Self::new()
    }
}

struct IpcRebroadcast {
    event_tx: tokio::sync::broadcast::Sender<String>,
}

/// Spawns the stream task: event fanout (disk + IPC + rendering) and command consumer.
pub fn spawn_stream_task(
    rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mode: StreamMode,
    log_path: std::path::PathBuf,
    ipc_handle: Option<IpcHandle>,
    approval_pending: Option<ApprovalPendingMap>,
    question_pending: Option<QuestionPendingMap>,
    cancel_tx: Option<oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    let qp = question_pending.clone();
    let (event_tx_ipc, command_rx) = match ipc_handle {
        Some(h) => {
            let (etx, crx) = h.into_parts();
            (Some(etx), Some(crx))
        }
        None => (None, None),
    };

    if let Some(crx) = command_rx {
        supervisor::spawn_command_consumer(crx, approval_pending, question_pending, cancel_tx);
    }

    spawn_event_fanout_task(rx, mode, log_path, event_tx_ipc, qp)
}

pub fn spawn_event_fanout_task(
    rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    mode: StreamMode,
    log_path: std::path::PathBuf,
    event_tx_ipc: Option<tokio::sync::broadcast::Sender<String>>,
    question_pending: Option<QuestionPendingMap>,
) -> tokio::task::JoinHandle<()> {
    let stats = StreamStats::new();

    let on_event: Option<EventEnvelopeFn> = match mode {
        StreamMode::Off => None,
        StreamMode::Ndjson => Some(Arc::new(|envelope: &EventEnvelope| {
            if let Ok(line) = serde_json::to_string(envelope) {
                println!("{line}");
            }
        })),
        StreamMode::Human => {
            let stats = stats.clone();
            Some(Arc::new(move |envelope: &EventEnvelope| {
                render_event(&envelope.event, &stats);
            }))
        }
    };

    let ipc_handle_rebuilt = event_tx_ipc.map(|tx| IpcRebroadcast { event_tx: tx });

    tokio::spawn(async move {
        use nca_common::event::EventEnvelope;
        use tokio::fs::OpenOptions;
        use tokio::io::AsyncWriteExt;

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .ok();

        let mut event_id: u64 = 0;
        let mut rx = rx;
        let qp = question_pending;
        while let Some(event) = rx.recv().await {
            event_id += 1;
            let envelope = EventEnvelope::new(event_id, event.clone());

            if let Some(ref ipc) = ipc_handle_rebuilt {
                let line = serde_json::to_string(&envelope).unwrap_or_default();
                let _ = ipc.event_tx.send(line);
            }

            if let Some(file) = log_file.as_mut()
                && let Ok(line) = serde_json::to_string(&envelope)
            {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }

            if let Some(ref cb) = on_event {
                cb(&envelope);
            }

            if let AgentEvent::QuestionRequested { ref question } = event
                && matches!(mode, StreamMode::Human)
                && std::io::stdin().is_terminal()
                && let Some(ref pending) = qp
            {
                let q = question.clone();
                let qid = q.question_id.clone();
                let pending = pending.clone();
                if let Ok(Ok(sel)) =
                    tokio::task::spawn_blocking(move || prompt_question_stdio(&q)).await
                    && let Ok(mut m) = pending.lock()
                    && let Some(tx) = m.remove(&qid)
                {
                    let _ = tx.send(sel);
                }
            }
        }
    })
}

// Claude Code-inspired color theme
mod theme {
    use colored::Color;

    pub const CLEAR_LINE: &str = "\x1B[2K";

    pub const USER_BG: Color = Color::TrueColor {
        r: 0,
        g: 145,
        b: 191,
    };
    pub const ASSISTANT_BG: Color = Color::TrueColor {
        r: 137,
        g: 87,
        b: 220,
    };
    pub const TOOL_BG: Color = Color::TrueColor {
        r: 58,
        g: 170,
        b: 214,
    };

    pub const SUCCESS: Color = Color::TrueColor {
        r: 63,
        g: 185,
        b: 80,
    };
    pub const ERROR: Color = Color::TrueColor {
        r: 248,
        g: 81,
        b: 73,
    };
    pub const WARNING: Color = Color::TrueColor {
        r: 210,
        g: 153,
        b: 34,
    };

    pub const TEXT: Color = Color::TrueColor {
        r: 220,
        g: 220,
        b: 230,
    };
    pub const TEXT_DIM: Color = Color::TrueColor {
        r: 150,
        g: 150,
        b: 160,
    };
}

/// Render a single event with Claude Code-like styling
fn render_event(event: &AgentEvent, stats: &StreamStats) {
    match event {
        AgentEvent::SessionStarted {
            session_id: _,
            model,
            workspace: _,
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!(
                "{} {}",
                "Connected to".color(theme::SUCCESS),
                model.color(theme::TEXT)
            );
            println!();
        }
        AgentEvent::TokensStreamed { delta } => {
            print!("{delta}");
            stats.record_output_token();
        }
        AgentEvent::ToolCallStarted {
            tool,
            input: _,
            call_id: _,
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!(
                "  {} {}",
                "⚡".color(theme::TOOL_BG).bold(),
                tool.to_uppercase().color(theme::TOOL_BG)
            );
        }
        AgentEvent::ToolCallCompleted { call_id: _, output } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            if output.success {
                println!(
                    "  {} {}",
                    "✓".color(theme::SUCCESS),
                    "Tool completed".color(theme::TEXT_DIM)
                );
            } else {
                println!(
                    "  {} {}",
                    "✗".color(theme::ERROR),
                    output
                        .error
                        .as_deref()
                        .unwrap_or("Tool failed")
                        .color(theme::ERROR)
                );
            }
        }
        AgentEvent::ApprovalRequested {
            call_id: _,
            tool,
            description,
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!(
                "  {} {}: {}",
                "?".color(theme::WARNING).bold(),
                tool.color(theme::WARNING),
                description.color(theme::TEXT_DIM)
            );
        }
        AgentEvent::ApprovalResolved {
            call_id: _,
            approved,
        } => {
            print!("{}", theme::CLEAR_LINE);
            if *approved {
                println!(
                    "  {} {}",
                    "✓".color(theme::SUCCESS),
                    "Approved".color(theme::SUCCESS)
                );
            } else {
                println!(
                    "  {} {}",
                    "✗".color(theme::ERROR),
                    "Denied".color(theme::ERROR)
                );
            }
        }
        AgentEvent::QuestionRequested { question } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!(
                "  {} {}",
                "?".color(theme::WARNING).bold(),
                question.prompt.color(theme::TEXT)
            );
            println!(
                "    {} {}",
                "[0]".color(theme::SUCCESS),
                format!("suggested: {}", question.suggested_answer).color(theme::TEXT_DIM)
            );
            for (i, o) in question.options.iter().enumerate() {
                println!(
                    "    {} {} — {}",
                    format!("[{}]", i + 1).color(theme::TOOL_BG),
                    o.id.color(theme::TEXT_DIM),
                    o.label.color(theme::TEXT)
                );
            }
            if question.allow_custom {
                println!("    {}", "[c] custom text".color(theme::TEXT_DIM));
            }
            println!();
        }
        AgentEvent::QuestionResolved {
            question_id,
            selection,
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!(
                "  {} question {} answered: {:?}",
                "✓".color(theme::SUCCESS),
                question_id.color(theme::TEXT_DIM),
                selection
            );
        }
        AgentEvent::CostUpdated {
            input_tokens,
            output_tokens,
            estimated_cost_usd,
        } => {
            stats.update_cost(
                *input_tokens,
                *output_tokens,
                (*estimated_cost_usd * 100.0) as u64,
            );
        }
        AgentEvent::SessionEnded { reason } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!();
            println!("  Session ended ({reason:?})");
            println!(
                "  {}/{} tokens, ${:.4}",
                stats.input_tokens(),
                stats.output_tokens(),
                stats.estimated_cost_usd()
            );
        }
        AgentEvent::Error { message } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            println!(
                "  {} {}",
                "✗".color(theme::ERROR),
                message.color(theme::ERROR)
            );
        }
        AgentEvent::ChildSessionSpawned {
            child_session_id,
            task,
            ..
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            let short = if child_session_id.len() > 8 {
                &child_session_id[..8]
            } else {
                child_session_id.as_str()
            };
            println!(
                "  {} sub-agent {}… — {}",
                "⚡".color(theme::TOOL_BG).bold(),
                short,
                task.chars().take(100).collect::<String>()
            );
        }
        AgentEvent::ChildSessionActivity {
            child_session_id,
            phase,
            detail,
        } => {
            print!("{}", theme::CLEAR_LINE);
            let short = if child_session_id.len() > 8 {
                &child_session_id[..8]
            } else {
                child_session_id.as_str()
            };
            println!(
                "  {} {}… · {} · {}",
                "↳".color(theme::TOOL_BG),
                short,
                phase.color(theme::TEXT_DIM),
                detail.color(theme::TEXT_DIM)
            );
        }
        AgentEvent::ChildSessionCompleted {
            child_session_id,
            status,
            ..
        } => {
            print!("{}", theme::CLEAR_LINE);
            println!();
            let short = if child_session_id.len() > 8 {
                &child_session_id[..8]
            } else {
                child_session_id.as_str()
            };
            println!(
                "  {} sub-agent {}… {}",
                "✓".color(theme::SUCCESS),
                short,
                status.color(theme::TEXT_DIM)
            );
        }
        AgentEvent::MessageReceived { role, content } => {
            println!();
            let header = match role.as_str() {
                "user" => format!(" {} ", "YOU".to_uppercase())
                    .on_color(theme::USER_BG)
                    .white()
                    .bold(),
                "assistant" => format!(" {} ", "nca")
                    .on_color(theme::ASSISTANT_BG)
                    .white()
                    .bold(),
                _ => format!(" {} ", role.to_uppercase())
                    .on_color(theme::WARNING)
                    .white()
                    .bold(),
            };
            println!("{header}");
            println!();
            for line in content.lines() {
                if line.trim().is_empty() {
                    println!();
                } else if line.starts_with("```") {
                    println!("{}", line.color(theme::TEXT_DIM));
                } else {
                    println!("{}", line.color(theme::TEXT));
                }
            }
        }
        _ => {}
    }
}

/// Public wrapper for rendering a single event (used by main.rs)
pub fn render_human_event(event: &AgentEvent) {
    let stats = StreamStats::new();
    render_event(event, &stats);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nca_common::event::{InteractiveQuestionPayload, QuestionOption};

    #[test]
    fn render_question_requested_does_not_panic() {
        let ev = AgentEvent::QuestionRequested {
            question: InteractiveQuestionPayload {
                question_id: "q-1".into(),
                call_id: "c".into(),
                prompt: "Test?".into(),
                options: vec![QuestionOption {
                    id: "x".into(),
                    label: "X".into(),
                }],
                allow_custom: true,
                suggested_answer: "X".into(),
            },
        };
        render_human_event(&ev);
    }
}
