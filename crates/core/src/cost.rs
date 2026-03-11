/// Tracks token usage and estimates cost for a session.
#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl CostTracker {
    pub fn add(&mut self, input: u64, output: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
    }

    /// Rough cost estimate in USD based on Claude Sonnet pricing.
    /// Real implementation should look up per-model pricing.
    pub fn estimated_cost_usd(&self) -> f64 {
        let input_cost = self.input_tokens as f64 * 3.0 / 1_000_000.0;
        let output_cost = self.output_tokens as f64 * 15.0 / 1_000_000.0;
        input_cost + output_cost
    }
}
