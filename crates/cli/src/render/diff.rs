/// Renders colored diffs in the terminal.
pub struct DiffRenderer;

impl DiffRenderer {
    pub fn new() -> Self {
        Self
    }

    /// Render a unified diff between old and new content.
    pub fn render(&self, old: &str, new: &str) -> String {
        let diff = similar::TextDiff::from_lines(old, new);
        diff.unified_diff()
            .context_radius(3)
            .header("before", "after")
            .to_string()
    }
}
