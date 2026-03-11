/// Renders the status bar: model name, mode, token cost.
pub struct StatusBar;

impl StatusBar {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, _model: &str, _safe_mode: bool, _cost_usd: f64) -> String {
        // TODO: ratatui widget for bottom status bar
        String::new()
    }
}
