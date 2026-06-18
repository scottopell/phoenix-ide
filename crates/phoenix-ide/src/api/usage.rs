//! Usage analytics endpoints.
//!
//! Serves `GET /api/usage` (the aggregate dashboard) and
//! `GET /api/usage/conversation/:id` (per-conversation drill-down). Both are
//! computed from the `turn_usage` table, which records token counts and the
//! model per LLM turn. This surface is purely token-oriented — it carries no
//! notion of monetary cost. A token-to-cost layer, if ever wanted, can map
//! these counts downstream without touching the persistence or query layers.
//!
//! Token counts cross the wire as `f64` rather than `u64`: every realistic
//! count is well under 2^53, so the value is exact, and the UI gets plain
//! `number`s for charting instead of `bigint`.

// Token counts cross the wire as f64 (see module docs); every count is < 2^53,
// so the i64/usize -> f64 casts here are exact, not lossy.
#![allow(clippy::cast_precision_loss)]

use super::AppState;
use crate::llm::all_models;
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

/// Aggregated token counts and turn count for one scope (a day, a model, a
/// provider, a project, a conversation, or a rolling window).
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct Totals {
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_write_tokens: f64,
    pub cache_read_tokens: f64,
    pub total_tokens: f64,
    pub turns: f64,
}

impl Totals {
    fn add(&mut self, input: i64, output: i64, cw: i64, cr: i64, turns: i64) {
        self.input_tokens += input as f64;
        self.output_tokens += output as f64;
        self.cache_write_tokens += cw as f64;
        self.cache_read_tokens += cr as f64;
        self.total_tokens += (input + output + cw + cr) as f64;
        self.turns += turns as f64;
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
    pub model: String,
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_write_tokens: f64,
    pub cache_read_tokens: f64,
    pub total_tokens: f64,
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
/// not in the registry.
fn provider_display(model_id: &str) -> String {
    all_models().iter().find(|m| m.id == model_id).map_or_else(
        || "Unknown".to_string(),
        |m| m.provider.display_name().to_string(),
    )
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
        daily_map
            .entry(row.day.clone())
            .or_default()
            .add(i, o, cw, cr, t);
        model_map
            .entry(row.model.clone())
            .or_default()
            .add(i, o, cw, cr, t);
        provider_map
            .entry(provider_display(&row.model))
            .or_default()
            .add(i, o, cw, cr, t);

        windows.all.add(i, o, cw, cr, t);
        if row.day.as_str() >= month_start.as_str() {
            windows.month.add(i, o, cw, cr, t);
        }
        if row.day.as_str() >= week_start.as_str() {
            windows.week.add(i, o, cw, cr, t);
        }
        if row.day == today {
            windows.today.add(i, o, cw, cr, t);
        }
    }

    let daily: Vec<DailyUsage> = daily_map
        .into_iter()
        .map(|(day, totals)| DailyUsage { day, totals })
        .collect();

    let mut by_model: Vec<ModelUsage> = model_map
        .into_iter()
        .map(|(model, totals)| ModelUsage {
            provider: provider_display(&model),
            model,
            totals,
        })
        .collect();
    by_model.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));

    let mut by_provider: Vec<ProviderUsage> = provider_map
        .into_iter()
        .map(|(provider, totals)| ProviderUsage { provider, totals })
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
        acc.totals.add(i, o, cw, cr, t);
        if row.started_at < acc.started_at {
            acc.started_at.clone_from(&row.started_at);
        }
        project_map
            .entry(row.project_id.clone())
            .or_default()
            .add(i, o, cw, cr, t);
    }

    let mut conversations: Vec<ConversationUsageRow> = conv_map
        .into_iter()
        .map(|(id, a)| ConversationUsageRow {
            root_conversation_id: id,
            label: a.label,
            slug: a.slug,
            project_id: a.project_id,
            worktree: a.worktree,
            started_at: a.started_at,
            totals: a.totals,
        })
        .collect();
    conversations.sort_by(|a, b| b.totals.total_tokens.total_cmp(&a.totals.total_tokens));
    conversations.truncate(MAX_CONVERSATIONS);

    let mut by_project: Vec<ProjectUsage> = project_map
        .into_iter()
        .map(|(project_id, totals)| ProjectUsage { project_id, totals })
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
    let rows = match state.db.usage_conversation_turns(&id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, conv_id = %id, "usage_conversation_turns failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "usage query failed").into_response();
        }
    };

    let mut totals = Totals::default();
    let turns: Vec<TurnPoint> = rows
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            totals.add(
                r.input_tokens,
                r.output_tokens,
                r.cache_creation_tokens,
                r.cache_read_tokens,
                1,
            );
            TurnPoint {
                index: idx as f64,
                created_at: r.created_at.clone(),
                model: r.model.clone(),
                input_tokens: r.input_tokens as f64,
                output_tokens: r.output_tokens as f64,
                cache_write_tokens: r.cache_creation_tokens as f64,
                cache_read_tokens: r.cache_read_tokens as f64,
                total_tokens: (r.input_tokens
                    + r.output_tokens
                    + r.cache_creation_tokens
                    + r.cache_read_tokens) as f64,
            }
        })
        .collect();

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
        t.add(1_000_000, 0, 0, 0, 1);
        t.add(0, 500_000, 250_000, 1_000_000, 1);
        assert_eq!(t.input_tokens, 1_000_000.0);
        assert_eq!(t.output_tokens, 500_000.0);
        assert_eq!(t.total_tokens, 2_750_000.0);
        assert_eq!(t.turns, 2.0);
    }
}
