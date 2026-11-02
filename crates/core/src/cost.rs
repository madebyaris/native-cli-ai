//! Token accounting and cost estimates, priced per model.

use std::collections::BTreeMap;

use nca_common::pricing::ModelPricing;

/// Flat list prices in USD per million tokens, from the vendors' pricing pages (fetched
/// 2026-11-02). Models whose price depends on prompt length are left out on purpose. Price them
/// with `[pricing."<model>"]` in config instead.
///
/// Matching uses the lowercase name with any vendor prefix (`openai/`) removed. A pattern matches
/// the exact name or the name followed by `-`, so dated snapshots and `-latest` aliases match.
/// Put more specific patterns first.
const BUILTIN_PRICES: &[(&str, ModelPricing)] = &[
    // Anthropic: base input, cache read, 5-minute cache write, output.
    ("claude-opus-5-5", price(4.0, 20.0, Some(0.20), Some(5.0))),
    ("claude-opus-5", price(5.0, 25.0, Some(0.50), Some(6.25))),
    ("claude-opus-4-8", price(5.0, 25.0, Some(0.50), Some(6.25))),
    ("claude-opus-4-7", price(5.0, 25.0, Some(0.50), Some(6.25))),
    ("claude-opus-4-6", price(5.0, 25.0, Some(0.50), Some(6.25))),
    ("claude-opus-4-5", price(5.0, 25.0, Some(0.50), Some(6.25))),
    (
        "claude-sonnet-5-5",
        price(2.0, 10.0, Some(0.10), Some(2.50)),
    ),
    ("claude-sonnet-5", price(2.0, 10.0, Some(0.20), Some(2.50))),
    ("claude-sonnet-4", price(3.0, 15.0, Some(0.30), Some(3.75))),
    ("claude-haiku-4-5", price(1.0, 5.0, Some(0.10), Some(1.25))),
    ("claude-3-5-haiku", price(0.80, 4.0, Some(0.08), Some(1.00))),
    // MiniMax: prompt caching read only, no write charge listed.
    ("minimax-m3.1-flash", price(0.15, 0.60, Some(0.015), None)),
    // OpenAI Standard tier. Cache reads are free to write, so no write price.
    ("gpt-4o-2024-05-13", price(5.00, 15.00, None, None)),
    ("gpt-4o-mini", price(0.15, 0.60, Some(0.075), None)),
    ("gpt-4o", price(2.50, 10.00, Some(1.25), None)),
    ("gpt-4.1-nano", price(0.10, 0.40, Some(0.025), None)),
    ("gpt-4.1-mini", price(0.40, 1.60, Some(0.10), None)),
    ("gpt-4.1", price(2.00, 8.00, Some(0.50), None)),
];

const fn price(
    input: f64,
    output: f64,
    cached: Option<f64>,
    cache_write: Option<f64>,
) -> ModelPricing {
    ModelPricing {
        input_per_mtok: input,
        output_per_mtok: output,
        cached_input_per_mtok: cached,
        cache_write_per_mtok: cache_write,
    }
}

/// The price for `model`: a configured override first, then the built-in table.
/// `None` means the price is unknown, so that usage adds no cost estimate.
pub fn price_for(model: &str, overrides: &BTreeMap<String, ModelPricing>) -> Option<ModelPricing> {
    if let Some((_, configured)) = overrides
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(model))
    {
        return Some(*configured);
    }
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    BUILTIN_PRICES
        .iter()
        .find(|(pattern, _)| matches_model(&name, pattern))
        .map(|(_, price)| *price)
}

fn matches_model(name: &str, pattern: &str) -> bool {
    name.strip_prefix(pattern)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

/// Token counts for one provider call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Whole prompt, including the cached and cache-written parts.
    pub input: u64,
    /// Part of `input` served from the provider's cache.
    pub cached_input: u64,
    /// Part of `input` written to the cache by this call.
    pub cache_write_input: u64,
    /// Generated tokens.
    pub output: u64,
}

/// Running token totals and cost estimate for a session.
#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    /// Whole prompt tokens, cached parts included.
    pub input_tokens: u64,
    /// Prompt tokens served from cache. Never more than `input_tokens`.
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    /// USD for the usage that had a known price.
    cost_usd: f64,
}

impl CostTracker {
    /// Record one provider call. `price` is the model's price at the time of the call, or
    /// `None` when that model has no known price (its tokens are counted, its cost is not).
    pub fn add(&mut self, usage: TokenUsage, price: Option<ModelPricing>) {
        let input = usage.input;
        let cached = usage.cached_input.min(input);
        let cache_write = usage.cache_write_input.min(input - cached);
        self.input_tokens += input;
        self.cached_input_tokens += cached;
        self.output_tokens += usage.output;
        if let Some(price) = price {
            self.cost_usd += cost_usd(input, cached, cache_write, usage.output, &price);
        }
    }

    /// Estimated USD, summed over the calls that had a known price. Zero when none did.
    pub fn estimated_cost_usd(&self) -> f64 {
        self.cost_usd
    }

    /// Share of prompt tokens served from cache, in whole percent. `None` before any prompt.
    pub fn cache_hit_percent(&self) -> Option<u64> {
        cache_hit_percent(self.input_tokens, self.cached_input_tokens)
    }
}

/// USD for one call. Uncached prompt tokens bill at the input price; cache reads and writes bill
/// at their own price when the model has one.
fn cost_usd(input: u64, cached: u64, cache_write: u64, output: u64, price: &ModelPricing) -> f64 {
    let uncached = input - cached - cache_write;
    let cached_rate = price.cached_input_per_mtok.unwrap_or(price.input_per_mtok);
    let write_rate = price.cache_write_per_mtok.unwrap_or(price.input_per_mtok);
    let usd = uncached as f64 * price.input_per_mtok
        + cached as f64 * cached_rate
        + cache_write as f64 * write_rate
        + output as f64 * price.output_per_mtok;
    usd / 1_000_000.0
}

/// Share of prompt tokens served from cache, in whole percent. `None` before any prompt tokens.
pub fn cache_hit_percent(input: u64, cached: u64) -> Option<u64> {
    (input > 0).then(|| cached.min(input) * 100 / input)
}

/// Cost for display: `$0.0123`, or `n/a` when tokens were used but none had a known price.
pub fn format_cost_usd(usd: f64, tokens: u64) -> String {
    if usd <= 0.0 && tokens > 0 {
        "n/a".to_string()
    } else {
        format!("${usd:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeMap<String, ModelPricing> {
        BTreeMap::new()
    }

    fn flat(input: f64, output: f64) -> ModelPricing {
        price(input, output, None, None)
    }

    #[test]
    fn builtin_prices_match_names_snapshots_and_vendor_prefixes() {
        let table = none();
        assert_eq!(
            price_for("claude-sonnet-5-5", &table).and_then(|p| p.cached_input_per_mtok),
            Some(0.10)
        );
        // A dated snapshot keeps its family's price.
        assert_eq!(
            price_for("claude-sonnet-5-20260101", &table).and_then(|p| p.cached_input_per_mtok),
            Some(0.20)
        );
        assert!(price_for("claude-sonnet-4-5-20250929", &table).is_some());
        assert!(price_for("claude-3-5-haiku-latest", &table).is_some());
        assert_eq!(
            price_for("openai/gpt-4o-mini", &table),
            price_for("gpt-4o-mini", &table)
        );
    }

    #[test]
    fn more_specific_names_win_over_their_prefixes() {
        let table = none();
        let input = |model: &str| price_for(model, &table).map(|p| p.input_per_mtok);
        assert_eq!(input("gpt-4o-mini-2024-07-18"), Some(0.15));
        assert_eq!(input("gpt-4o-2024-08-06"), Some(2.50));
        assert_eq!(input("gpt-4o-2024-05-13"), Some(5.00));
        assert_eq!(input("gpt-4.1-nano"), Some(0.10));
        assert_eq!(input("claude-opus-5-5"), Some(4.0));
        assert_eq!(input("claude-opus-5"), Some(5.0));
    }

    #[test]
    fn unlisted_and_tiered_models_have_no_price() {
        let table = none();
        // Tiered by prompt length: must not inherit a flat price.
        assert_eq!(price_for("gpt-6-luna", &table), None);
        assert_eq!(price_for("claude-haiku-5-5", &table), None);
        assert_eq!(price_for("MiniMax-M3", &table), None);
        assert_eq!(price_for("deepseek-v4-1-flash", &table), None);
        // Sharing a prefix is not the same model.
        assert_eq!(price_for("gpt-4omni", &table), None);
    }

    #[test]
    fn config_override_wins_and_matches_case_insensitively() {
        let mut configured = BTreeMap::new();
        configured.insert("gpt-6-luna".to_string(), flat(0.10, 0.50));
        configured.insert("claude-sonnet-5-5".to_string(), flat(9.0, 9.0));

        assert_eq!(price_for("GPT-6-Luna", &configured), Some(flat(0.10, 0.50)));
        assert_eq!(
            price_for("claude-sonnet-5-5", &configured).map(|p| p.input_per_mtok),
            Some(9.0)
        );
    }

    #[test]
    fn cached_reads_bill_at_the_cached_rate() {
        let sonnet = price(3.0, 15.0, Some(0.30), None);
        let mut tracker = CostTracker::default();
        tracker.add(
            TokenUsage {
                input: 1_000_000,
                cached_input: 800_000,
                cache_write_input: 0,
                output: 100_000,
            },
            Some(sonnet),
        );

        // 200k uncached at $3, 800k cached at $0.30, 100k output at $15.
        let expected = 0.6 + 0.24 + 1.5;
        assert!((tracker.estimated_cost_usd() - expected).abs() < 1e-9);
        assert_eq!(tracker.cache_hit_percent(), Some(80));
    }

    #[test]
    fn cache_writes_bill_at_their_own_rate() {
        let anthropic = price(2.0, 10.0, Some(0.10), Some(2.50));
        let mut tracker = CostTracker::default();
        tracker.add(
            TokenUsage {
                input: 950_000,
                cached_input: 800_000,
                cache_write_input: 50_000,
                output: 0,
            },
            Some(anthropic),
        );

        // 100k uncached at $2, 50k written at $2.50, 800k read at $0.10.
        assert!((tracker.estimated_cost_usd() - 0.405).abs() < 1e-9);
    }

    #[test]
    fn missing_write_and_cached_prices_default_to_the_input_price() {
        let mut tracker = CostTracker::default();
        tracker.add(
            TokenUsage {
                input: 1_000_000,
                cached_input: 500_000,
                cache_write_input: 250_000,
                output: 0,
            },
            Some(flat(4.0, 0.0)),
        );

        assert!((tracker.estimated_cost_usd() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn unpriced_usage_counts_tokens_but_adds_no_cost() {
        let mut tracker = CostTracker::default();
        tracker.add(
            TokenUsage {
                input: 500,
                cached_input: 100,
                cache_write_input: 0,
                output: 20,
            },
            None,
        );

        assert_eq!(tracker.input_tokens, 500);
        assert_eq!(tracker.cached_input_tokens, 100);
        assert_eq!(tracker.output_tokens, 20);
        assert_eq!(tracker.estimated_cost_usd(), 0.0);
        assert_eq!(
            format_cost_usd(tracker.estimated_cost_usd(), tracker.input_tokens),
            "n/a"
        );
    }

    #[test]
    fn cached_counts_never_exceed_the_prompt() {
        let mut tracker = CostTracker::default();
        tracker.add(
            TokenUsage {
                input: 100,
                cached_input: 150,
                cache_write_input: 50,
                output: 0,
            },
            None,
        );

        assert_eq!(tracker.cached_input_tokens, 100);
        assert_eq!(tracker.cache_hit_percent(), Some(100));
    }

    #[test]
    fn display_helpers_handle_empty_and_partial_values() {
        assert_eq!(cache_hit_percent(0, 0), None);
        assert_eq!(cache_hit_percent(1000, 999), Some(99));
        assert_eq!(format_cost_usd(0.00234, 1100), "$0.0023");
        assert_eq!(format_cost_usd(0.0, 0), "$0.0000");
    }
}
