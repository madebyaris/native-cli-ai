use nca_common::config::ModelPricing;

/// Tracks token usage and estimates cost for a session.
///
/// `pricing` is the per-model USD-per-million-tokens table, set at construction
/// time. The tracker keeps running totals and computes cost on demand.
#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub pricing: ModelPricing,
}

impl CostTracker {
    pub fn new(pricing: ModelPricing) -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            pricing,
        }
    }

    pub fn add(&mut self, input: u64, output: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
    }

    /// Cost estimate in USD, using the per-model pricing captured at construction.
    /// Sessions with unknown pricing fall back to `ModelPricing::default()`
    /// (zero), matching the conservative "report only what we know" behaviour.
    pub fn estimated_cost_usd(&self) -> f64 {
        let input_cost = self.input_tokens as f64 * self.pricing.input_per_million / 1_000_000.0;
        let output_cost = self.output_tokens as f64 * self.pricing.output_per_million / 1_000_000.0;
        input_cost + output_cost
    }

    pub fn set_pricing(&mut self, pricing: ModelPricing) {
        self.pricing = pricing;
    }
}
