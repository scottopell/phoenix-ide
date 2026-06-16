//! Per-model token pricing and cost computation.
//!
//! Pricing is a lookup table keyed by model id. A model that is not in the
//! table is *unpriced*: [`cost_for`] returns `None` rather than `0`, so the
//! UI can distinguish "this cost nothing" from "we don't know what this cost".
//! The `OpenAI` (`gpt-*`) models have no published rates wired in here and are
//! intentionally unpriced — their token counts still surface; only the dollar
//! figure is withheld.
//!
//! Rates are dollars per **million** tokens. Cache rates follow the Anthropic
//! schedule and are derived from the input rate: a cache *write* costs 1.25× the
//! input rate (5-minute TTL) and a cache *read* costs 0.1× the input rate.

use phoenix_core::domain::llm_types::Usage;

/// Cache-write multiplier over the base input rate (5-minute ephemeral TTL).
const CACHE_WRITE_MULT: f64 = 1.25;
/// Cache-read multiplier over the base input rate.
const CACHE_READ_MULT: f64 = 0.10;

/// Dollars-per-million-token rates for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// Uncached input tokens, $/Mtok.
    pub input_per_mtok: f64,
    /// Output tokens, $/Mtok.
    pub output_per_mtok: f64,
}

impl ModelPricing {
    const fn new(input_per_mtok: f64, output_per_mtok: f64) -> Self {
        Self {
            input_per_mtok,
            output_per_mtok,
        }
    }

    fn cache_write_per_mtok(self) -> f64 {
        self.input_per_mtok * CACHE_WRITE_MULT
    }

    fn cache_read_per_mtok(self) -> f64 {
        self.input_per_mtok * CACHE_READ_MULT
    }
}

/// Cost of a single usage record, broken out by token class. All figures are
/// US dollars. Kept separate (rather than collapsed to a total) so the UI can
/// show *where* spend goes — cache writes vs reads vs fresh input vs output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

impl Cost {
    pub fn total(self) -> f64 {
        self.input + self.output + self.cache_write + self.cache_read
    }
}

/// Pricing for a model id, or `None` if the model is unpriced.
///
/// The id is the registry id stored in `turn_usage.model` (e.g.
/// `claude-opus-4-8`), not the provider `api_name`.
pub fn pricing_for(model_id: &str) -> Option<ModelPricing> {
    // Rates per the Anthropic pricing schedule ($/Mtok input / output).
    let p = match model_id {
        "claude-fable-5" | "claude-mythos-5" => ModelPricing::new(10.0, 50.0),
        "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" | "claude-opus-4-5" => {
            ModelPricing::new(5.0, 25.0)
        }
        "claude-sonnet-4-6" | "claude-sonnet-4-5" => ModelPricing::new(3.0, 15.0),
        "claude-haiku-4-5" => ModelPricing::new(1.0, 5.0),
        // `OpenAI` gpt-* and anything else: no published rates wired in → unpriced.
        _ => return None,
    };
    Some(p)
}

/// Whether a model id has wired-in pricing.
pub fn is_priced(model_id: &str) -> bool {
    pricing_for(model_id).is_some()
}

/// Compute the cost of a usage record for a model, or `None` if unpriced.
#[allow(clippy::cast_precision_loss)] // token counts are < 2^53; f64 is exact here
pub fn cost_for(model_id: &str, usage: &Usage) -> Option<Cost> {
    let p = pricing_for(model_id)?;
    let per_mtok = |tokens: u64, rate: f64| (tokens as f64) * rate / 1_000_000.0;
    Some(Cost {
        input: per_mtok(usage.input_tokens, p.input_per_mtok),
        output: per_mtok(usage.output_tokens, p.output_per_mtok),
        cache_write: per_mtok(usage.cache_creation_tokens, p.cache_write_per_mtok()),
        cache_read: per_mtok(usage.cache_read_tokens, p.cache_read_per_mtok()),
    })
}

/// Compute cost from raw aggregate token counts for a model, or `None` if
/// unpriced. Convenience for SQL-aggregated rows that arrive as `i64` sums
/// (negative inputs are clamped to 0).
#[allow(clippy::cast_sign_loss)] // each value is clamped to >= 0 before the cast
pub fn cost_from_totals(
    model_id: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> Option<Cost> {
    let usage = Usage {
        input_tokens: input_tokens.max(0) as u64,
        output_tokens: output_tokens.max(0) as u64,
        cache_creation_tokens: cache_creation_tokens.max(0) as u64,
        cache_read_tokens: cache_read_tokens.max(0) as u64,
    };
    cost_for(model_id, &usage)
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact-rate assertions on small, exactly-representable values
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cw: u64, cr: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: cw,
            cache_read_tokens: cr,
        }
    }

    #[test]
    fn opus_input_output_rates() {
        // 1M input @ $5, 1M output @ $25.
        let c = cost_for("claude-opus-4-8", &usage(1_000_000, 1_000_000, 0, 0)).unwrap();
        assert!((c.input - 5.0).abs() < 1e-9);
        assert!((c.output - 25.0).abs() < 1e-9);
        assert!((c.total() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn cache_rates_derived_from_input() {
        // cache write = 1.25x input, cache read = 0.1x input.
        let c = cost_for("claude-opus-4-8", &usage(0, 0, 1_000_000, 1_000_000)).unwrap();
        assert!((c.cache_write - 6.25).abs() < 1e-9);
        assert!((c.cache_read - 0.5).abs() < 1e-9);
    }

    #[test]
    fn sonnet_and_haiku_rates() {
        let s = cost_for("claude-sonnet-4-6", &usage(1_000_000, 0, 0, 0)).unwrap();
        assert!((s.input - 3.0).abs() < 1e-9);
        let h = cost_for("claude-haiku-4-5", &usage(0, 1_000_000, 0, 0)).unwrap();
        assert!((h.output - 5.0).abs() < 1e-9);
    }

    #[test]
    fn openai_models_are_unpriced() {
        assert!(cost_for("gpt-5.5", &usage(1_000_000, 1_000_000, 0, 0)).is_none());
        assert!(cost_for("gpt-5.4-mini", &usage(1_000_000, 0, 0, 0)).is_none());
        assert!(!is_priced("gpt-5.5"));
        assert!(is_priced("claude-opus-4-8"));
    }

    #[test]
    fn unknown_model_unpriced() {
        assert!(cost_for("totally-made-up", &usage(100, 100, 0, 0)).is_none());
    }

    #[test]
    fn totals_helper_clamps_negative() {
        let c = cost_from_totals("claude-opus-4-8", -5, 1_000_000, 0, 0).unwrap();
        assert!((c.input - 0.0).abs() < 1e-9);
        assert!((c.output - 25.0).abs() < 1e-9);
    }

    #[test]
    fn zero_usage_zero_cost() {
        let c = cost_for("claude-opus-4-8", &usage(0, 0, 0, 0)).unwrap();
        assert_eq!(c.total(), 0.0);
    }
}
