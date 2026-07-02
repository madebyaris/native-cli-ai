//! Transcript component — renders DisplayBlock items with virtual scrolling,
//! text selection, streaming text, and collapsible blocks.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

use nca_common::event::{AgentEvent, InteractiveQuestionPayload, QuestionSelection};
use nca_common::tool::ToolResult;

use crate::tui::state::{ApprovalRequest, DisplayBlock};
use crate::tui::text_utils::{format_tool_input_for_display, short_session_prefix, truncate};

// ── Theme (from shared module, re-exported via searchable_list) ───
use super::searchable_list::theme;

use super::transcript_render::{
    apply_selection_highlight, block_line_count, emit_block_lines, emit_empty_fallback_lines,
    emit_streaming_assistant_lines, emit_streaming_reasoning_lines, plain_text_from_lines,
    wrap_text,
};

const MOUSE_SCROLL_LINES: usize = 6;

// ── Public types ──────────────────────────────────────────────────

/// Actions triggered by clicking transcript lines.
#[derive(Debug, Clone)]
pub(crate) enum TranscriptHit {
    Question(QuestionSelection),
    ToggleThinking(usize),
    ToggleStreamingThinking,
    ToggleToolOutput(usize),
}

/// External side-effects from transcript interactions.
#[derive(Debug)]
pub(crate) enum TranscriptAction {
    /// No side-effect.
    None,
    /// User answered a question by clicking an option.
    QuestionAnswer(QuestionSelection),
    /// Copy text to clipboard + push System message about it.
    CopyToClipboard(String),
    /// Push a System message into blocks.
    PushSystem(String),
    /// Push an ErrorLine message into blocks.
    PushError(String),
}

/// Per flattened transcript line: click selects this answer or toggles thinking.
pub(crate) type LineAnswerHit = Option<TranscriptHit>;

// ── BlockLineCache ─────────────────────────────────────────────────

/// Cached per-block line counts + cumulative offsets for fast virtualization.
/// Keyed by (blocks_generation, width) so it auto-invalidates.
pub(crate) struct BlockLineCache {
    generation: u64,
    width: u16,
    /// `heights[i]` = line count of `blocks[i]`.  Same length as `blocks`.
    heights: Vec<usize>,
    /// `cum_offsets[i]` = total lines of blocks[0..i].  Length = heights.len() + 1.
    cum_offsets: Vec<usize>,
}

impl BlockLineCache {
    fn new() -> Self {
        Self {
            generation: 0,
            width: 0,
            heights: Vec::new(),
            cum_offsets: vec![0],
        }
    }

    /// Returns `true` when the cache is valid for the given generation + width.
    fn is_valid(&self, g: u64, w: u16) -> bool {
        self.generation == g && self.width == w && !self.heights.is_empty()
    }

    /// Rebuild from `blocks`.  Must be called when `is_valid` returns `false`.
    fn rebuild(&mut self, blocks: &[DisplayBlock], g: u64, w: u16) {
        self.generation = g;
        self.width = w;
        self.heights.clear();
        self.cum_offsets.clear();
        let w_usize = w as usize;
        for b in blocks {
            self.heights.push(block_line_count(b, w_usize));
        }
        self.cum_offsets.reserve(self.heights.len() + 1);
        let mut acc = 0usize;
        self.cum_offsets.push(acc);
        for &h in &self.heights {
            acc += h;
            self.cum_offsets.push(acc);
        }
    }

    /// Total committed-block line count (excluding streaming).
    #[inline]
    fn total(&self) -> usize {
        *self.cum_offsets.last().unwrap_or(&0)
    }
}

// ── TranscriptState ──────────────────────────────────────────────

pub(crate) struct TranscriptState {
    // ── Transcript data ──
    pub(crate) blocks: Vec<DisplayBlock>,
    pub(crate) streaming_assistant: Option<String>,
    pub(crate) streaming_reasoning: Option<String>,
    pub(crate) streaming_reasoning_expanded: bool,
    pub(crate) blocks_generation: u64,

    // ── Scroll state ──
    pub(crate) scroll_lines: usize,
    pub(crate) transcript_follow_tail: bool,

    // ── Text selection ──
    pub(crate) transcript_selection: Option<((usize, usize), (usize, usize))>, // ((line, col), (line, col))
    pub(crate) transcript_dragging: bool,
    pub(crate) transcript_drag_anchor: Option<(usize, usize)>,

    // ── Cache ──
    pub(crate) line_cache: BlockLineCache,
    pub(crate) last_visible_hits: Vec<LineAnswerHit>,

    // ── Active question for answer routing ──
    pub(crate) _active_question: Option<InteractiveQuestionPayload>,
}

impl TranscriptState {
    pub(crate) fn new() -> Self {
        Self {
            blocks: Vec::new(),
            streaming_assistant: None,
            streaming_reasoning: None,
            streaming_reasoning_expanded: false,
            blocks_generation: 0,
            scroll_lines: 0,
            transcript_follow_tail: true,
            transcript_selection: None,
            transcript_dragging: false,
            transcript_drag_anchor: None,
            line_cache: BlockLineCache::new(),
            last_visible_hits: Vec::new(),
            _active_question: None,
        }
    }

    // ── Event handling ──────────────────────────────────────────

    /// Process an AgentEvent and return any side-effect actions.
    pub(crate) fn apply_event(&mut self, e: &AgentEvent) -> TranscriptAction {
        match e {
            AgentEvent::SessionStarted { .. } => {
                // Model/session/branch are StatusBar concerns
            }
            AgentEvent::MessageReceived { role, content } => {
                if role == "user" {
                    self.streaming_assistant = None;
                    self.blocks.push(DisplayBlock::User(content.clone()));
                } else if role == "assistant" {
                    self.streaming_assistant = None;
                    // Commit any accumulated reasoning before the assistant text.
                    if let Some(reasoning) = self.streaming_reasoning.take()
                        && !reasoning.trim().is_empty()
                    {
                        self.blocks.push(DisplayBlock::Thinking {
                            content: reasoning,
                            expanded: false,
                        });
                    }
                    self.blocks.push(DisplayBlock::Assistant(content.clone()));
                }
            }
            AgentEvent::TokensStreamed { delta } => {
                self.streaming_assistant
                    .get_or_insert_with(String::new)
                    .push_str(delta);
            }
            AgentEvent::ReasoningStreamed { delta } => {
                let s = self.streaming_reasoning.get_or_insert(String::new());
                s.push_str(delta);
            }
            AgentEvent::ToolCallStarted {
                call_id,
                tool,
                input,
            } => {
                self.flush_stream_before_tool();
                self.blocks.push(DisplayBlock::ToolRunning {
                    name: tool.clone(),
                    call_id: call_id.clone(),
                    input: format_tool_input_for_display(tool, input),
                });
            }
            AgentEvent::ToolCallCompleted { call_id, output } => {
                let ok = output.success;
                // On failure, prefer the explicit error string; fall back to the
                // tool's stdout/stderr output (e.g. run_validation leaves error
                // as None and puts diagnostics in output).
                let full_output = if ok {
                    output.output.clone()
                } else {
                    output
                        .error
                        .clone()
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| output.output.clone())
                };
                let detail = truncate(&full_output, 120);
                if let Some(idx) = self.blocks.iter().rposition(|b| {
                    matches!(b, DisplayBlock::ToolRunning { call_id: id, .. } if id == call_id)
                        || matches!(b, DisplayBlock::ApprovalPending(req) if req.call_id == *call_id)
                }) {
                    let name = match &self.blocks[idx] {
                        DisplayBlock::ToolRunning { name, .. } => name.clone(),
                        DisplayBlock::ApprovalPending(req) => req.tool.clone(),
                        _ => "?".into(),
                    };
                    let input = match &self.blocks[idx] {
                        DisplayBlock::ToolRunning { input, .. } => input.clone(),
                        _ => String::new(),
                    };
                    self.blocks[idx] = DisplayBlock::ToolDone {
                        name,
                        input,
                        ok,
                        detail,
                        full_output,
                        expanded: false,
                    };
                } else {
                    self.blocks.push(DisplayBlock::ToolDone {
                        name: "?".into(),
                        input: String::new(),
                        ok,
                        detail,
                        full_output,
                        expanded: false,
                    });
                }
            }
            AgentEvent::ApprovalRequested {
                call_id,
                tool,
                description,
            } => {
                let input = self
                    .blocks
                    .iter()
                    .rev()
                    .find_map(|block| match block {
                        DisplayBlock::ToolRunning {
                            call_id: id, input, ..
                        } if id == call_id => Some(input.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "{}".into());
                let req = ApprovalRequest {
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    description: description.clone(),
                    input,
                };
                if let Some(idx) = self.blocks.iter().rposition(
                    |b| matches!(b, DisplayBlock::ToolRunning { call_id: id, .. } if id == call_id),
                ) {
                    self.blocks[idx] = DisplayBlock::ApprovalPending(req);
                } else {
                    self.blocks.push(DisplayBlock::ApprovalPending(req));
                }
            }
            AgentEvent::ApprovalResolved {
                call_id,
                approved,
                allow_pattern: _,
            } => {
                // Replace the matching ApprovalPending block in-place so the
                // stale prompt ("y/yes approve · n/no deny") disappears.
                if let Some(idx) = self.blocks.iter().rposition(|block| {
                    matches!(
                        block,
                        DisplayBlock::ApprovalPending(req) if req.call_id == *call_id
                    )
                }) {
                    let tool = match &self.blocks[idx] {
                        DisplayBlock::ApprovalPending(req) => req.tool.clone(),
                        _ => "tool".into(),
                    };
                    self.blocks[idx] = DisplayBlock::ApprovalResolved {
                        tool,
                        approved: *approved,
                    };
                } else {
                    self.blocks.push(DisplayBlock::ApprovalResolved {
                        tool: "tool".into(),
                        approved: *approved,
                    });
                }
            }
            AgentEvent::QuestionRequested { question } => {
                self.blocks.push(DisplayBlock::Question(question.clone()));
                self.transcript_follow_tail = true;
            }
            AgentEvent::QuestionResolved {
                question_id,
                selection,
            } => {
                self.blocks.push(DisplayBlock::System(format!(
                    "Answered question {question_id}: {selection:?}"
                )));
            }
            AgentEvent::Error { message } => {
                self.blocks.push(DisplayBlock::ErrorLine(message.clone()));
            }
            AgentEvent::ChildSessionSpawned {
                child_session_id,
                task,
                ..
            } => {
                let short = short_session_prefix(child_session_id);
                self.blocks.push(DisplayBlock::System(format!(
                    "Sub-agent {short}… — {}",
                    truncate(task, 80)
                )));
            }
            AgentEvent::ChildSessionActivity {
                child_session_id,
                phase,
                detail,
            } => {
                let short = short_session_prefix(child_session_id);
                let d = truncate(detail, 120);
                self.blocks
                    .push(DisplayBlock::System(format!("↳ {short}… · {phase} · {d}")));
            }
            AgentEvent::ChildSessionCompleted {
                child_session_id,
                status,
                ..
            } => {
                let short = short_session_prefix(child_session_id);
                self.blocks.push(DisplayBlock::System(format!(
                    "Sub-agent {short}… done: {status}"
                )));
            }
            AgentEvent::CostUpdated { .. }
            | AgentEvent::ContextStatsUpdated { .. }
            | AgentEvent::BusyStateChanged { .. }
            | AgentEvent::Checkpoint { .. } => {
                // StatusBar concerns — ignore
            }
            _ => {}
        }
        // Bump generation so cached block-line-heights are invalidated.
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
        TranscriptAction::None
    }

    fn flush_stream_before_tool(&mut self) {
        if let Some(reasoning) = self.streaming_reasoning.take()
            && !reasoning.trim().is_empty()
        {
            self.blocks.push(DisplayBlock::Thinking {
                content: reasoning,
                expanded: false,
            });
        }
        if let Some(s) = self.streaming_assistant.take()
            && !s.trim().is_empty()
        {
            self.blocks.push(DisplayBlock::Assistant(s));
        }
    }

    pub(crate) fn push_error(&mut self, msg: String) {
        self.blocks.push(DisplayBlock::ErrorLine(msg));
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }

    pub(crate) fn push_system(&mut self, msg: String) {
        self.blocks.push(DisplayBlock::System(msg));
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }

    pub(crate) fn push_blocks(&mut self, blocks: Vec<DisplayBlock>) {
        self.blocks.extend(blocks);
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }

    pub(crate) fn set_streaming_assistant(&mut self, text: Option<String>) {
        self.streaming_assistant = text;
    }

    pub(crate) fn set_streaming_reasoning(&mut self, text: Option<String>) {
        self.streaming_reasoning = text;
    }

    pub(crate) fn clear(&mut self) {
        self.blocks.clear();
        self.streaming_assistant = None;
        self.streaming_reasoning = None;
        self.streaming_reasoning_expanded = false;
        self.scroll_lines = 0;
        self.transcript_follow_tail = true;
        self.transcript_selection = None;
        self.transcript_dragging = false;
        self.transcript_drag_anchor = None;
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }

    /// Store the current active approval request for rendering.
    pub(crate) fn set_active_approval(&mut self, _req: Option<ApprovalRequest>) {
        // Phase 3a: approval rendering handled by existing DisplayBlock::ApprovalPending
    }

    /// Store the current active question for rendering and answer routing.
    pub(crate) fn set_active_question(&mut self, q: Option<InteractiveQuestionPayload>) {
        // Track for answer routing via active_question_id()
        self._active_question = q;
    }

    /// Return the question_id of the currently active question (for answer routing).
    pub(crate) fn active_question_id(&self) -> Option<String> {
        self._active_question
            .as_ref()
            .map(|q| q.question_id.clone())
    }

    /// Toggle expanded state of a specific ToolDone block.
    pub(crate) fn toggle_tool_output(&mut self, block_index: usize) {
        if let Some(DisplayBlock::ToolDone { expanded, .. }) = self.blocks.get_mut(block_index) {
            *expanded = !*expanded;
            self.blocks_generation = self.blocks_generation.wrapping_add(1);
        }
    }

    /// Toggle all ToolDone blocks: expand if any collapsed, collapse all otherwise.
    pub(crate) fn toggle_all_tool_output(&mut self) {
        let any_collapsed = self.blocks.iter().any(|b| {
            matches!(
                b,
                DisplayBlock::ToolDone {
                    expanded: false,
                    ..
                }
            )
        });
        for block in self.blocks.iter_mut() {
            if let DisplayBlock::ToolDone { expanded, .. } = block {
                *expanded = any_collapsed;
            }
        }
        let msg = if any_collapsed {
            "tool output expanded"
        } else {
            "tool output collapsed"
        };
        self.blocks
            .push(DisplayBlock::System(format!("[tool-output] {msg}")));
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }

    // ── Key handling ─────────────────────────────────────────────

    pub(crate) fn handle_key(&mut self, key: KeyEvent, area: Rect) -> TranscriptAction {
        match key.code {
            KeyCode::PageUp => {
                let th = area.height.saturating_sub(2) as usize;
                let page = th.saturating_sub(1).max(1);
                self.transcript_follow_tail = false;
                self.scroll_lines = self.scroll_lines.saturating_sub(page);
            }
            KeyCode::PageDown => {
                let inner_w = area.width.saturating_sub(2);
                let total = self.total_line_count(inner_w);
                let th = area.height.saturating_sub(2) as usize;
                let max_scroll = total.saturating_sub(th);
                let page = th.saturating_sub(1).max(1);
                self.scroll_lines = (self.scroll_lines + page).min(max_scroll);
                if self.scroll_lines >= max_scroll {
                    self.transcript_follow_tail = true;
                }
            }
            KeyCode::End => {
                self.transcript_follow_tail = true;
            }
            _ => {}
        }
        TranscriptAction::None
    }

    // ── Mouse handling ──────────────────────────────────────────

    pub(crate) fn handle_mouse(
        &mut self,
        event: &MouseEvent,
        area: Rect,
        total_lines: usize,
    ) -> TranscriptAction {
        let content_area = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let inside = event.column >= content_area.x
            && event.column < content_area.x + content_area.width
            && event.row >= content_area.y
            && event.row < content_area.y + content_area.height;
        if !inside {
            return TranscriptAction::None;
        }

        let th = content_area.height as usize;
        let max_scroll = total_lines.saturating_sub(th);

        let gline = (event.row - content_area.y) as usize + self.scroll_lines;
        let gcol = (event.column - content_area.x) as usize;

        match event.kind {
            MouseEventKind::ScrollUp => {
                self.transcript_selection = None;
                self.transcript_dragging = false;
                self.transcript_follow_tail = false;
                self.scroll_lines = self.scroll_lines.saturating_sub(MOUSE_SCROLL_LINES);
            }
            MouseEventKind::ScrollDown => {
                self.transcript_selection = None;
                self.transcript_dragging = false;
                self.scroll_lines = (self.scroll_lines + MOUSE_SCROLL_LINES).min(max_scroll);
                if self.scroll_lines >= max_scroll {
                    self.transcript_follow_tail = true;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Handle transcript hit: toggle thinking or answer question.
                let vis_start = self.scroll_lines;
                let local_idx = gline.saturating_sub(vis_start);
                if local_idx < self.last_visible_hits.len() {
                    match &self.last_visible_hits[local_idx] {
                        Some(TranscriptHit::ToggleThinking(block_idx)) => {
                            if let Some(DisplayBlock::Thinking { expanded, .. }) =
                                self.blocks.get_mut(*block_idx)
                            {
                                *expanded = !*expanded;
                                self.blocks_generation = self.blocks_generation.wrapping_add(1);
                            }
                            self.transcript_selection = None;
                            self.transcript_dragging = false;
                            return TranscriptAction::None;
                        }
                        Some(TranscriptHit::ToggleStreamingThinking) => {
                            self.streaming_reasoning_expanded = !self.streaming_reasoning_expanded;
                            self.transcript_selection = None;
                            self.transcript_dragging = false;
                            return TranscriptAction::None;
                        }
                        Some(TranscriptHit::ToggleToolOutput(block_idx)) => {
                            if let Some(DisplayBlock::ToolDone { expanded, .. }) =
                                self.blocks.get_mut(*block_idx)
                            {
                                *expanded = !*expanded;
                                self.blocks_generation = self.blocks_generation.wrapping_add(1);
                            }
                            self.transcript_selection = None;
                            self.transcript_dragging = false;
                            return TranscriptAction::None;
                        }
                        Some(TranscriptHit::Question(sel)) => {
                            self.transcript_selection = None;
                            self.transcript_dragging = false;
                            return TranscriptAction::QuestionAnswer(sel.clone());
                        }
                        None => {}
                    }
                }
                // No question hit — start a new text selection.
                let click_pos = (gline, gcol);
                self.transcript_selection = Some((click_pos, click_pos));
                self.transcript_drag_anchor = Some((gline, gcol));
                self.transcript_dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.transcript_dragging => {
                if let Some((anchor_line, anchor_col)) = self.transcript_drag_anchor {
                    if self.transcript_selection.is_some() {
                        let start = (
                            anchor_line.min(gline),
                            if anchor_line <= gline {
                                anchor_col
                            } else {
                                gcol
                            },
                        );
                        let end = (
                            anchor_line.max(gline),
                            if anchor_line <= gline {
                                gcol
                            } else {
                                anchor_col
                            },
                        );
                        self.transcript_selection = Some((start, end));
                    } else {
                        // First drag attempt after Down — filter spurious large jumps.
                        let line_dist = gline.abs_diff(anchor_line);
                        let col_dist = gcol.abs_diff(anchor_col);
                        if line_dist <= 1 && col_dist <= 3 {
                            let start = (
                                anchor_line.min(gline),
                                if anchor_line <= gline {
                                    anchor_col
                                } else {
                                    gcol
                                },
                            );
                            let end = (
                                anchor_line.max(gline),
                                if anchor_line <= gline {
                                    gcol
                                } else {
                                    anchor_col
                                },
                            );
                            self.transcript_selection = Some((start, end));
                        }
                    }
                } else if let Some(((anchor_line, _), _)) = self.transcript_selection {
                    let start = (anchor_line.min(gline), 0);
                    let end = (anchor_line.max(gline), gcol);
                    self.transcript_selection = Some((start.min(end), start.max(end)));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.transcript_dragging = false;
                self.transcript_drag_anchor = None;
                // Auto-copy selected text to clipboard on mouse-up.
                if let Some((sel_start, sel_end)) = self.transcript_selection {
                    let (sl, sc) = sel_start;
                    let (el, ec) = sel_end;
                    if sl < el || (sl == el && sc != ec) {
                        let inner_w = area.width.saturating_sub(2);
                        let all_lines = self.build_all_lines(inner_w);
                        let text = plain_text_from_lines(&all_lines, sel_start, sel_end);
                        let n = text.trim_end_matches('\n').chars().count();
                        match crate::image_attach::copy_text_to_clipboard(&text) {
                            Ok(()) => {
                                return TranscriptAction::PushSystem(format!(
                                    "Copied {n} chars to clipboard"
                                ));
                            }
                            Err(e) => {
                                return TranscriptAction::PushError(format!(
                                    "Clipboard failed: {e}"
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        TranscriptAction::None
    }

    // ── Line counting helpers ───────────────────────────────────

    fn streaming_reasoning_line_count(&self, w: usize) -> usize {
        if let Some(reasoning) = &self.streaming_reasoning
            && !reasoning.is_empty()
        {
            let all_rl = wrap_text(reasoning, w);
            let total_rl = all_rl.len();
            let preview_rl = 5usize;
            let show_rl = if self.streaming_reasoning_expanded || total_rl <= preview_rl {
                total_rl
            } else {
                preview_rl
            };
            let mut rl = 1 + show_rl + 1;
            if total_rl > preview_rl {
                rl += 1;
            }
            return rl;
        }
        0
    }

    fn streaming_assistant_line_count(&self, w: usize) -> usize {
        if let Some(stream) = &self.streaming_assistant
            && !stream.is_empty()
        {
            return 2 + wrap_text(stream, w).len();
        }
        0
    }

    pub(crate) fn total_line_count(&mut self, width: u16) -> usize {
        let w = width.max(20) as usize;
        let mut n = if self.line_cache.is_valid(self.blocks_generation, width) {
            self.line_cache.total()
        } else {
            self.line_cache
                .rebuild(&self.blocks, self.blocks_generation, width);
            self.line_cache.total()
        };

        // Streaming reasoning block
        n += self.streaming_reasoning_line_count(w);

        // Streaming assistant block
        n += self.streaming_assistant_line_count(w);

        // Empty state fallback
        if n == 0 && self.blocks.is_empty() {
            n = 4;
        }

        n
    }

    // ── Line building ───────────────────────────────────────────

    fn build_visible_lines(
        &mut self,
        width: u16,
        area_height: usize,
    ) -> (Vec<Line<'static>>, Vec<LineAnswerHit>) {
        let w = width.max(20) as usize;
        // Ensure cache is up to date.
        if !self.line_cache.is_valid(self.blocks_generation, width) {
            self.line_cache
                .rebuild(&self.blocks, self.blocks_generation, width);
        }
        let blocks_total = self.line_cache.total();

        let srl = self.streaming_reasoning_line_count(w);
        let sal = self.streaming_assistant_line_count(w);
        let ef = if blocks_total == 0 && self.blocks.is_empty() {
            4usize
        } else {
            0
        };
        let total = blocks_total + srl + sal + ef;

        let start = self.scroll_lines;
        let end = (start + area_height).min(total);

        if start >= end || start >= total {
            self.last_visible_hits = Vec::new();
            return (Vec::new(), Vec::new());
        }

        let cap = (end - start).min(200);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(cap);
        let mut hits: Vec<LineAnswerHit> = Vec::with_capacity(cap);
        let mut global_line = 0usize;

        for (bi, block) in self.blocks.iter().enumerate() {
            let bh = self.line_cache.heights[bi];
            let block_end = global_line + bh;
            if block_end <= start {
                global_line = block_end;
                continue;
            }
            if global_line >= end {
                break;
            }
            emit_block_lines(
                block,
                bi,
                w,
                &mut lines,
                &mut hits,
                start.saturating_sub(global_line),
                end.saturating_sub(global_line.max(start)),
            );
            global_line = block_end;
        }

        // Streaming reasoning
        if srl > 0 && global_line < end {
            emit_streaming_reasoning_lines(
                self,
                w,
                &mut lines,
                &mut hits,
                start.saturating_sub(global_line),
                end.saturating_sub(global_line.max(start)),
            );
            global_line += srl;
        }

        // Streaming assistant
        if sal > 0 && global_line < end {
            emit_streaming_assistant_lines(
                self,
                w,
                &mut lines,
                &mut hits,
                start.saturating_sub(global_line),
                end.saturating_sub(global_line.max(start)),
            );
            global_line += sal;
        }

        // Empty fallback
        if ef > 0 && global_line < end {
            emit_empty_fallback_lines(
                &mut lines,
                &mut hits,
                start.saturating_sub(global_line),
                end.saturating_sub(global_line.max(start)),
            );
        }

        self.last_visible_hits = hits.clone();
        (lines, hits)
    }

    /// Build ALL lines (for clipboard copy — non-virtualized).
    fn build_all_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let w = width.max(20) as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut hits: Vec<LineAnswerHit> = Vec::new();

        for (bi, block) in self.blocks.iter().enumerate() {
            emit_block_lines(block, bi, w, &mut lines, &mut hits, 0, usize::MAX);
        }

        // Streaming reasoning
        emit_streaming_reasoning_lines(self, w, &mut lines, &mut hits, 0, usize::MAX);
        // Streaming assistant
        emit_streaming_assistant_lines(self, w, &mut lines, &mut hits, 0, usize::MAX);

        // Empty fallback
        if lines.is_empty() && self.blocks.is_empty() {
            emit_empty_fallback_lines(&mut lines, &mut hits, 0, usize::MAX);
        }

        lines
    }

    // ── Render ──────────────────────────────────────────────────

    pub(crate) fn render(&mut self, area: Rect) -> Paragraph<'static> {
        let inner_w = area.width.saturating_sub(2);
        let total = self.total_line_count(inner_w);
        let transcript_h = area.height.saturating_sub(2) as usize;

        // Clamp scroll position
        let max_scroll = total.saturating_sub(transcript_h);
        if self.transcript_follow_tail || self.scroll_lines > max_scroll {
            self.scroll_lines = max_scroll;
        }

        let (visible_lines, _hits) = self.build_visible_lines(inner_w, transcript_h);

        // Apply selection highlight
        let highlighted =
            apply_selection_highlight(visible_lines, self.scroll_lines, self.transcript_selection);

        let title = format!(" transcript — {total} lines ");

        Paragraph::new(Text::from(highlighted))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::BORDER))
                    .title(Span::styled(title, Style::default().fg(theme::MUTED))),
            )
            .style(Style::default().bg(theme::BG))
    }
}
