/// Renders the status bar: model name, mode, token cost.
pub struct StatusBar;

impl StatusBar {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, model: &str, safe_mode: bool, cost_usd: f64) -> String {
        let mode = if safe_mode { "safe" } else { "default" };
        format!("model={model} | mode={mode} | est_cost=${cost_usd:.4}")
    }
}
