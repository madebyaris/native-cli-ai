//! Shared state and helpers for searchable list popups.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
pub(crate) use ratatui::widgets::Clear as ClearWidget;

// ── Theme (re-export from shared module) ────────────────────────────

pub(crate) use super::theme::colors as theme;

// ── Centered rect helper ───────────────────────────────────────────

/// Compute a centered popup rect clamped to the terminal area.
pub(crate) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let popup_w = width
        .min(area.width.saturating_sub(2).max(20))
        .min(area.width);
    let popup_h = height
        .min(area.height.saturating_sub(2).max(6))
        .min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(popup_w) / 2,
        area.y + area.height.saturating_sub(popup_h) / 2,
        popup_w,
        popup_h,
    )
}

// ── SearchableListState ───────────────────────────────────────────

/// Shared state for list-picker popups with search filtering.
pub(crate) struct SearchableListState {
    pub query: String,
    pub index: usize,
    pub scroll: usize,
    pub max_rows: usize,
}

/// Action returned by `SearchableListState::handle_key`.
pub(crate) enum SearchableListAction {
    None,
    /// Select the item at the given absolute index in the full (unfiltered) list.
    Select(usize),
    Close,
}

impl SearchableListState {
    pub fn new(max_rows: usize) -> Self {
        Self {
            query: String::new(),
            index: 0,
            scroll: 0,
            max_rows,
        }
    }

    pub fn reset(&mut self) {
        self.query.clear();
        self.index = 0;
        self.scroll = 0;
    }

    /// Handle key events for a searchable list. `selectable_count` is the number
    /// of selectable (non-header) items in the filtered list.
    pub fn handle_key(&mut self, key: KeyEvent, selectable_count: usize) -> SearchableListAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => SearchableListAction::Close,
            (KeyCode::Up, _) => {
                if self.index > 0 {
                    self.index -= 1;
                }
                SearchableListAction::None
            }
            (KeyCode::Down, _) => {
                if selectable_count > 0 {
                    self.index = (self.index + 1).min(selectable_count - 1);
                }
                SearchableListAction::None
            }
            (KeyCode::Enter, _) => {
                if selectable_count > 0 {
                    SearchableListAction::Select(self.index.min(selectable_count - 1))
                } else {
                    SearchableListAction::None
                }
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                self.index = 0;
                self.scroll = 0;
                SearchableListAction::None
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.query.push(c);
                self.index = 0;
                self.scroll = 0;
                SearchableListAction::None
            }
            _ => SearchableListAction::None,
        }
    }

    /// Ensure the selected item is visible within the viewport.
    pub fn adjust_scroll(&mut self, total_filtered: usize) {
        if total_filtered == 0 {
            self.scroll = 0;
            return;
        }
        let viewport = total_filtered.min(self.max_rows);
        if self.index < self.scroll {
            self.scroll = self.index;
        } else if self.index >= self.scroll + viewport {
            self.scroll = self.index.saturating_sub(viewport - 1);
        }
        self.scroll = self.scroll.min(total_filtered.saturating_sub(viewport));
    }

    /// Compute the visible range `(start, end)` into the filtered list.
    pub fn visible_range(&self, total_filtered: usize) -> (usize, usize) {
        let viewport = total_filtered.min(self.max_rows);
        let start = self.scroll.min(total_filtered.saturating_sub(viewport));
        (start, (start + viewport).min(total_filtered))
    }
}
