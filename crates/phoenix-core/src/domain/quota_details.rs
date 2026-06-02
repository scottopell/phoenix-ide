//! Structured quota and rate-limit state extracted from the codex backend.
//!
//! Pure-data snapshots co-owned by the llm layer (which parses them from
//! `x-codex-*` response headers) and the state machine (`LlmOutcome::
//! UsageLimitReached` carries a `QuotaDetails` by value). They live in the
//! base crate so those layers depend *down* onto a common vocabulary instead
//! of onto each other.
//!
//! The HTTP header-parsing functions that produce these types stay in the llm
//! layer (`llm/rate_limit.rs`) since they depend on `reqwest`.
//!
//! These types intentionally mirror the codex CLI's `RateLimitSnapshot` shape
//! (`codex-rs/protocol/src/protocol.rs`) without depending on the
//! `codex_protocol` / `codex_api` crates — we only need the data layout.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Structured quota state extracted from the codex backend on 429 (header path)
/// or from a mid-stream `codex.rate_limits` SSE event.
///
/// All fields are optional: the codex backend populates a subset depending on
/// which limit was hit (per-model vs global), the user's plan, and whether
/// credits are tracked. Consumers must handle every field being `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct QuotaDetails {
    pub plan_type: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub credits: Option<CreditsSnapshot>,
    pub promo_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}
