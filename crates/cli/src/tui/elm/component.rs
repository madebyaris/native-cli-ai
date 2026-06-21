//! Component trait for type-level documentation.
//!
//! This trait documents the interface every component must implement.
//! Components are stored as concrete types in `Components` (enum-based dispatch),
//! so this trait is NOT used for runtime polymorphism (`dyn NcaComponent`).

use ratatui::Frame;
use ratatui::layout::Rect;

/// Interface every TUI component implements.
///
/// Components are stored as concrete fields in the `Components` struct.
/// The `CompId` enum provides runtime dispatch via match arms.
pub(crate) trait NcaComponent: Send {
    /// Render this component into the given frame area.
    fn view(&mut self, frame: &mut Frame, area: Rect);
}
