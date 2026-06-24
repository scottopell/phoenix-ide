//! Usage analytics endpoints.
//!
//! Serves `GET /api/usage` (the aggregate dashboard) and
//! `GET /api/usage/conversation/:id` (per-conversation drill-down). Both are
//! computed from the `turn_usage` table, which records token counts and the
//! model per LLM turn. Monetary cost is derived at presentation time from the
//! model id stored with each token row; it is not persisted.
//!
//! Token counts cross the wire as `f64` rather than `u64`: every realistic
//! count is well under 2^53, so the value is exact, and the UI gets plain
//! `number`s for charting instead of `bigint`.

// Token counts cross the wire as f64 (see module docs); every count is < 2^53,
// so the i64/usize -> f64 casts here are exact, not lossy.
#![allow(clippy::cast_precision_loss)]

use super::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{Duration, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use ts_rs::TS;

/// Estimated USD cost for a token aggregate. `estimated_usd` includes only rows
/// with known pricing; callers must show `pricing_known == false` / non-zero
/// `unknown_turns` so unpriced models are not mistaken for free usage.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CostSummary {
    pub estimated_usd: f64,
    pub pricing_known: bool,
    pub unknown_turns: f64,
}

impl Default for CostSummary {
    fn default() -> Self {
        Self {
            estimated_usd: 0.0,
            pricing_known: true,
            unknown_turns: 0.0,
        }
    }
}

impl CostSummary {
    fn add_known(&mut self, usd: f64) {
        self.estimated_usd += usd;
    }

    fn add_unknown(&mut self, turns: i64) {
        self.unknown_turns += turns as f64;
    }

    fn finish(&mut self) {
        self.pricing_known = self.unknown_turns == 0.0;
    }
}

/// Estimated per-category USD cost for one turn. Category fields are `None`
/// when the turn's model has no known pricing.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TurnCost {
    pub input_usd: Option<f64>,
    pub output_usd: Option<f64>,
    pub cache_write_usd: Option<f64>,
    pub cache_read_usd: Option<f64>,
    pub total_usd: Option<f64>,
    pub pricing_known: bool,
}

/// USD prices per 1M tokens for the token classes emitted by Phoenix.
#[derive(Debug, Clone, Copy)]
struct ModelPricing {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

impl ModelPricing {
    fn cost(self, input: i64, output: i64, cache_write: i64, cache_read: i64) -> TurnCost {
        let input_usd = tokens_to_usd(input, self.input);
        let output_usd = tokens_to_usd(output, self.output);
        let cache_write_usd = tokens_to_usd(cache_write, self.cache_write);
        let cache_read_usd = tokens_to_usd(cache_read, self.cache_read);
        TurnCost {
            input_usd: Some(input_usd),
            output_usd: Some(output_usd),
            cache_write_usd: Some(cache_write_usd),
            cache_read_usd: Some(cache_read_usd),
            total_usd: Some(input_usd + output_usd + cache_write_usd + cache_read_usd),
            pricing_known: true,
        }
    }
}

fn tokens_to_usd(tokens: i64, usd_per_million: f64) -> f64 {
    (tokens as f64 / 1_000_000.0) * usd_per_million
}

fn unknown_turn_cost() -> TurnCost {
    TurnCost {
        input_usd: None,
        output_usd: None,
        cache_write_usd: None,
        cache_read_usd: None,
        total_usd: None,
        pricing_known: false,
    }
}

fn model_pricing(model: &str) -> Option<ModelPricing> {
    match model {
        "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => Some(ModelPricing {
            input: 15.00,
            output: 75.00,
            cache_write: 18.75,
            cache_read: 1.50,
        }),
        "claude-sonnet-4-6" => Some(ModelPricing {
            input: 3.00,
            output: 15.00,
            cache_write: 3.75,
            cache_read: 0.30,
        }),
        "claude-haiku-4-5" => Some(ModelPricing {
            input: 0.80,
            output: 4.00,
            cache_write: 1.00,
            cache_read: 0.08,
        }),
        "gpt-5.5" => Some(ModelPricing {
            input: 5.00,
            output: 30.00,
            cache_write: 5.00,
            cache_read: 0.50,
        }),
        "gpt-5.4" => Some(ModelPricing {
            input: 2.50,
            output: 15.00,
            cache_write: 2.50,
            cache_read: 0.25,
        }),
        "gpt-5.4-mini" => Some(ModelPricing {
            input: 0.75,
            output: 4.50,
            cache_write: 0.75,
            cache_read: 0.075,
        }),
        "gpt-5.3-codex" => Some(ModelPricing {
            input: 1.25,
            output: 10.00,
            cache_write: 1.25,
            cache_read: 0.125,
        }),
        "mock" => Some(ModelPricing {
            input: 0.0,
            output: 0.0,
            cache_write: 0.0,
            cache_read: 0.0,
        }),
        _ => None,
    }
}

pub(crate) fn calculate_turn_cost(
    model: &str,
    input: i64,
    output: i64,
    cache_write: i64,
    cache_read: i64,
) -> TurnCost {
    model_pricing(model).map_or_else(unknown_turn_cost, |p| {
        p.cost(input, output, cache_write, cache_read)
    })
}

fn add_cost(summary: &mut CostSummary, cost: TurnCost, turns: i64) {
    if let Some(usd) = cost.total_usd {
        summary.add_known(usd);
    } else {
        summary.add_unknown(turns);
    }
}

/// Aggregated token counts, turn count, and derived estimated cost for one
/// scope (a day, a model, a provider, a project, a conversation, or a rolling window).
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct Totals {
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_write_tokens: f64,
    pub cache_read_tokens: f64,
    pub total_tokens: f64,
    pub turns: f64,
    pub cost: CostSummary,
}

impl Totals {
    fn add(&mut self, input: i64, output: i64, cw: i64, cr: i64, turns: i64, cost: TurnCost) {
        self.input_tokens += input as f64;
        self.output_tokens += output as f64;
        self.cache_write_tokens += cw as f64;
        self.cache_read_tokens += cr as f64;
        self.total_tokens += (input + output + cw + cr) as f64;
        self.turns += turns as f64;
        add_cost(&mut self.cost, cost, turns);
    }

    fn finish_cost(&mut self) {
        self.cost.finish();
    }
}

/// Token usage for the four rolling windows (UTC). `all` is unbounded.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct UsageWindows {
    pub today: Totals,
    pub week: Totals,
    pub month: Totals,
    pub all: Totals,
}

/// One day of the timeseries. `day` is a UTC `YYYY-MM-DD`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DailyUsage {
    pub day: String,
    pub totals: Totals,
}

/// Per-model rollup. `provider` is resolved from the model registry.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ModelUsage {
    pub model: String,
    pub provider: String,
    pub totals: Totals,
}

/// Per-provider rollup (provider resolved from the model registry).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProviderUsage {
    pub provider: String,
    pub totals: Totals,
}

/// Per-project rollup. `project_id` is `None` for conversations not attached to
/// a project.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProjectUsage {
    pub project_id: Option<String>,
    pub totals: Totals,
}

/// One conversation in the per-conversation list. `label` is the best available
/// human name (title, else slug, else id). `worktree` is extracted from the
/// conversation mode when present.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ConversationUsageRow {
    pub root_conversation_id: String,
    pub label: String,
    pub slug: Option<String>,
    pub project_id: Option<String>,
    pub worktree: Option<String>,
    pub started_at: String,
    pub totals: Totals,
}

/// One bucket of the tokens-per-turn histogram. `[lo, hi)` token range; `hi` is
/// `None` for the open-ended top bucket.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct HistogramBucket {
    pub lo: f64,
    pub hi: Option<f64>,
    pub count: f64,
}

/// The full `/api/usage` payload.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct UsageOverview {
    pub generated_at: String,
    pub windows: UsageWindows,
    pub daily: Vec<DailyUsage>,
    pub by_model: Vec<ModelUsage>,
    pub by_provider: Vec<ProviderUsage>,
    pub by_project: Vec<ProjectUsage>,
    pub conversations: Vec<ConversationUsageRow>,
    pub turn_token_histogram: Vec<HistogramBucket>,
}

/// One turn in the per-conversation drill-down.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TurnPoint {
    pub index: f64,
    pub created_at: String,
    pub first_byte_at: Option<String>,
    pub first_byte_latency_ms: Option<f64>,
    pub model: String,
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_write_tokens: f64,
    pub cache_read_tokens: f64,
    pub total_tokens: f64,
    pub cost: TurnCost,
}

/// The `/api/usage/conversation/:id` payload.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ConversationUsageDetail {
    pub root_conversation_id: String,
    pub totals: Totals,
    pub turns: Vec<TurnPoint>,
}

/// Resolve a model id to a provider display name, or `"Unknown"` if the id is
/// not in the live registry.
fn provider_display(state: &AppState, model_id: &str) -> String {
    state.llm_registry.provider_display_name(model_id)
}

/// Fixed token-count bucket edges for the tokens-per-turn histogram. The final
/// bucket is open-ended above the last edge.
const HISTOGRAM_EDGES: &[i64] = &[
    1_000, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000,
];

fn build_histogram(per_turn_totals: &[i64]) -> Vec<HistogramBucket> {
    let mut counts = vec![0f64; HISTOGRAM_EDGES.len() + 1];
    for &t in per_turn_totals {
        let idx = HISTOGRAM_EDGES
            .iter()
            .position(|&edge| t < edge)
            .unwrap_or(HISTOGRAM_EDGES.len());
        counts[idx] += 1.0;
    }
    let mut buckets = Vec::with_capacity(counts.len());
    let mut lo = 0i64;
    for (i, &count) in counts.iter().enumerate() {
        let hi = HISTOGRAM_EDGES.get(i).copied();
        buckets.push(HistogramBucket {
            lo: lo as f64,
            hi: hi.map(|h| h as f64),
            count,
        });
        if let Some(h) = hi {
            lo = h;
        }
    }
    buckets
}

/// Cap on the number of conversations returned (highest token use first).
const MAX_CONVERSATIONS: usize = 200;

/// Accumulator for one conversation's rollup while grouping the per-model rows.
struct ConvAcc {
    label: String,
    slug: Option<String>,
    project_id: Option<String>,
    worktree: Option<String>,
    started_at: String,
    totals: Totals,
}

/// `GET /api/usage` — assemble the aggregate usage dashboard.
#[allow(clippy::too_many_lines)] // one linear assembly of the dashboard payload
pub async fn usage_overview(State(state): State<AppState>) -> impl IntoResponse {
    let daily_rows = match state.db.usage_daily_by_model().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "usage_daily_by_model failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "usage query failed").into_response();
        }
    };
    let conv_rows = match state.db.usage_by_conversation().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "usage_by_conversation failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "usage query failed").into_response();
        }
    };
    let per_turn = match state.db.usage_turn_token_totals().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "usage_turn_token_totals failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "usage query failed").into_response();
        }
    };

    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let week_start = (now - Duration::days(6)).format("%Y-%m-%d").to_string();
    let month_start = (now - Duration::days(29)).format("%Y-%m-%d").to_string();

    let mut windows = UsageWindows::default();
    let mut daily_map: BTreeMap<String, Totals> = BTreeMap::new();
    let mut model_map: BTreeMap<String, Totals> = BTreeMap::new();
    let mut provider_map: BTreeMap<String, Totals> = BTreeMap::new();

    for row in &daily_rows {
        let (i, o, cw, cr, t) = (
            row.input_tokens,
            row.output_tokens,
            row.cache_creation_tokens,
            row.cache_read_tokens,
            row.turns,
        );
        let cost = calculate_turn_cost(&row.model, i, o, cw, cr);
        daily_map
            .entry(row.day.clone())
            .or_default()
            .add(i, o, cw, cr, t, cost);
        model_map
            .entry(row.model.clone())
            .or_default()
            .add(i, o, cw, cr, t, cost);
        provider_map
            .entry(provider_display(&state, &row.model))
            .or_default()
            .add(i, o, cw, cr, t, cost);

        windows.all.add(i, o, cw, cr, t, cost);
        if row.day.as_str() >= month_start.as_str() {
            windows.month.add(i, o, cw, cr, t, cost);
        }
        if row.day.as_str() >= week_start.as_str() {
            windows.week.add(i, o, cw, cr, t, cost);
        }
        if row.day == today {
            windows.today.add(i, o, cw, cr, t, cost);
        }
    }

    windows.today.finish_cost();
    windows.week.finish_cost();
    windows.month.finish_cost();
    windows.all.finish_cost();

    let daily: Vec<DailyUsage> = daily_map
        .into_iter()
        .map(|(day, mut totals)| {
            totals.finish_cost();
            DailyUsage { day, totals }
        })
        .collect();

    let mut by_model: Vec<ModelUsage> = model_map
        .into_iter()
        .map(|(model, mut totals)| {
            totals.finish_cost();
            ModelUsage {
                provider: provider_display(&state, &model),
                model,
                totals,
            }
        })
        .collect();
    by_model.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));

    let mut by_provider: Vec<ProviderUsage> = provider_map
        .into_iter()
        .map(|(provider, mut totals)| {
            totals.finish_cost();
            ProviderUsage { provider, totals }
        })
        .collect();
    by_provider.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));

    // Per-conversation and per-project rollups from the conversation rows.
    let mut conv_map: BTreeMap<String, ConvAcc> = BTreeMap::new();
    let mut project_map: BTreeMap<Option<String>, Totals> = BTreeMap::new();

    for row in &conv_rows {
        let (i, o, cw, cr, t) = (
            row.input_tokens,
            row.output_tokens,
            row.cache_creation_tokens,
            row.cache_read_tokens,
            row.turns,
        );
        let label = row
            .title
            .clone()
            .or_else(|| row.slug.clone())
            .unwrap_or_else(|| row.root_conversation_id.clone());
        let acc = conv_map
            .entry(row.root_conversation_id.clone())
            .or_insert_with(|| ConvAcc {
                label,
                slug: row.slug.clone(),
                project_id: row.project_id.clone(),
                worktree: row.worktree_path.clone(),
                started_at: row.started_at.clone(),
                totals: Totals::default(),
            });
        let cost = calculate_turn_cost(&row.model, i, o, cw, cr);
        acc.totals.add(i, o, cw, cr, t, cost);
        if row.started_at < acc.started_at {
            acc.started_at.clone_from(&row.started_at);
        }
        project_map
            .entry(row.project_id.clone())
            .or_default()
            .add(i, o, cw, cr, t, cost);
    }

    let mut conversations: Vec<ConversationUsageRow> = conv_map
        .into_iter()
        .map(|(id, mut a)| {
            a.totals.finish_cost();
            ConversationUsageRow {
                root_conversation_id: id,
                label: a.label,
                slug: a.slug,
                project_id: a.project_id,
                worktree: a.worktree,
                started_at: a.started_at,
                totals: a.totals,
            }
        })
        .collect();
    conversations.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));
    conversations.truncate(MAX_CONVERSATIONS);

    let mut by_project: Vec<ProjectUsage> = project_map
        .into_iter()
        .map(|(project_id, mut totals)| {
            totals.finish_cost();
            ProjectUsage { project_id, totals }
        })
        .collect();
    by_project.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));

    let overview = UsageOverview {
        generated_at: now.to_rfc3339(),
        windows,
        daily,
        by_model,
        by_provider,
        by_project,
        conversations,
        turn_token_histogram: build_histogram(&per_turn),
    };

    Json(overview).into_response()
}

/// `GET /api/usage/conversation/:id` — per-conversation drill-down. `:id` is the
/// root conversation id; sub-agent turns are included.
pub async fn usage_conversation_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let turns_projection =
        match crate::analytics::project_usage_turns_for_root(&state.db, &id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, conv_id = %id, "usage analytics projection failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "usage query failed").into_response();
            }
        };

    let mut totals = Totals::default();
    let turns: Vec<TurnPoint> = turns_projection
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            totals.add(
                r.tokens.input_tokens,
                r.tokens.output_tokens,
                r.tokens.cache_creation_tokens,
                r.tokens.cache_read_tokens,
                1,
                r.cost,
            );
            TurnPoint {
                index: idx as f64,
                created_at: r.created_at.to_rfc3339(),
                first_byte_at: r.first_byte_at.map(|t| t.to_rfc3339()),
                first_byte_latency_ms: r.first_byte_latency_ms.map(|ms| ms as f64),
                model: r.model.clone(),
                input_tokens: r.tokens.input_tokens as f64,
                output_tokens: r.tokens.output_tokens as f64,
                cache_write_tokens: r.tokens.cache_creation_tokens as f64,
                cache_read_tokens: r.tokens.cache_read_tokens as f64,
                total_tokens: (r.tokens.input_tokens
                    + r.tokens.output_tokens
                    + r.tokens.cache_creation_tokens
                    + r.tokens.cache_read_tokens) as f64,
                cost: r.cost,
            }
        })
        .collect();
    totals.finish_cost();

    Json(ConversationUsageDetail {
        root_conversation_id: id,
        totals,
        turns,
    })
    .into_response()
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact counts on small, exactly-representable values
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_by_edge() {
        let totals = vec![0, 500, 1_500, 30_000, 2_000_000];
        let h = build_histogram(&totals);
        assert_eq!(h.len(), HISTOGRAM_EDGES.len() + 1);
        // 0 and 500 fall in [0, 1000)
        assert_eq!(h[0].count, 2.0);
        assert_eq!(h[0].lo, 0.0);
        assert_eq!(h[0].hi, Some(1000.0));
        // 1500 falls in [1000, 5000)
        assert_eq!(h[1].count, 1.0);
        // 2_000_000 falls in the open-ended top bucket
        assert_eq!(h.last().unwrap().count, 1.0);
        assert_eq!(h.last().unwrap().hi, None);
    }

    #[test]
    fn totals_sum_tokens_and_turns() {
        let mut t = Totals::default();
        let known = calculate_turn_cost("claude-sonnet-4-6", 1_000_000, 0, 0, 0);
        let unknown = calculate_turn_cost("unpriced-model", 0, 500_000, 250_000, 1_000_000);
        t.add(1_000_000, 0, 0, 0, 1, known);
        t.add(0, 500_000, 250_000, 1_000_000, 1, unknown);
        t.finish_cost();
        assert_eq!(t.input_tokens, 1_000_000.0);
        assert_eq!(t.output_tokens, 500_000.0);
        assert_eq!(t.total_tokens, 2_750_000.0);
        assert_eq!(t.turns, 2.0);
        assert_eq!(t.cost.estimated_usd, 3.0);
        assert_eq!(t.cost.unknown_turns, 1.0);
        assert!(!t.cost.pricing_known);
    }

    #[test]
    fn cost_calculation_prices_each_token_category() {
        let cost = calculate_turn_cost(
            "claude-sonnet-4-6",
            1_000_000,
            2_000_000,
            3_000_000,
            4_000_000,
        );
        assert!(cost.pricing_known);
        assert_eq!(cost.input_usd, Some(3.0));
        assert_eq!(cost.output_usd, Some(30.0));
        assert_eq!(cost.cache_write_usd, Some(11.25));
        assert_eq!(cost.cache_read_usd, Some(1.2));
        assert_eq!(cost.total_usd, Some(45.45));
    }

    #[test]
    fn unknown_model_pricing_is_not_zero_cost() {
        let cost = calculate_turn_cost("future-model", 1_000_000, 1_000_000, 0, 0);
        assert!(!cost.pricing_known);
        assert_eq!(cost.total_usd, None);

        let mut totals = Totals::default();
        totals.add(1_000_000, 1_000_000, 0, 0, 1, cost);
        totals.finish_cost();
        assert_eq!(totals.cost.estimated_usd, 0.0);
        assert_eq!(totals.cost.unknown_turns, 1.0);
        assert!(!totals.cost.pricing_known);
    }

    #[test]
    fn mixed_model_aggregation_sums_per_model_costs() {
        let mut totals = Totals::default();
        totals.add(
            1_000_000,
            0,
            0,
            0,
            1,
            calculate_turn_cost("claude-sonnet-4-6", 1_000_000, 0, 0, 0),
        );
        totals.add(
            0,
            1_000_000,
            0,
            0,
            1,
            calculate_turn_cost("claude-haiku-4-5", 0, 1_000_000, 0, 0),
        );
        totals.finish_cost();

        assert_eq!(totals.cost.estimated_usd, 7.0);
        assert_eq!(totals.cost.unknown_turns, 0.0);
        assert!(totals.cost.pricing_known);
    }
}
