/// Renders colored diffs in the terminal.
pub struct DiffRenderer;

impl DiffRenderer {
    pub fn new() -> Self {
        Self
    }

    /// Render a unified diff between old and new content.
    pub fn render(&self, _old: &str, _new: &str) -> String {
        // TODO: use `similar` crate for diff generation, ratatui styled spans for output
        String::new()
    }
}
