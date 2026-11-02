use serde::{Deserialize, Serialize};

/// Flat list price for one model, in USD per million tokens.
///
/// Only flat prices belong here. Models whose price depends on prompt length are
/// configured explicitly rather than approximated.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Price of prompt tokens that were not served from or written to a cache.
    pub input_per_mtok: f64,
    /// Price of generated tokens.
    pub output_per_mtok: f64,
    /// Price of prompt tokens served from the provider's cache. Defaults to the input price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<f64>,
    /// Price of prompt tokens written to the cache (Anthropic bills these above the input
    /// price). Defaults to the input price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<f64>,
}
