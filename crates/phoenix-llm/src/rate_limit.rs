//! Structured quota and rate-limit state extracted from the codex backend.
//!
//! The codex backend (`chatgpt.com/backend-api/codex`) returns plan and quota
//! state in `x-codex-*` response headers on every Responses call — both 200
//! and 429. The full set lives in headers: window snapshots
//! (`primary`/`secondary` used-percent / window-minutes / reset-at), plan
//! type (`x-codex-plan-type`), active limit (`x-codex-active-limit`),
//! credits (`x-codex-credits-*`), promo message (`x-codex-promo-message`).
//! The 429 JSON body additionally provides `resets_at` (no header
//! equivalent) and historically also `plan_type` (now also in a header).
//!
//! Phoenix obtains the authoritative account snapshot from Codex's authenticated
//! usage endpoint. HTTP response headers and WebSocket `codex.rate_limits`
//! events provide partial per-turn updates that are normalized into the same shape.
//!
//! These types intentionally mirror the codex CLI's `RateLimitSnapshot` shape
//! (`codex-rs/protocol/src/protocol.rs`) without depending on the
//! `codex_protocol` / `codex_api` crates — we only need the data layout.
//!
//! The pure-data snapshot types (`QuotaDetails`, `RateLimitWindow`,
//! `CreditsSnapshot`) live in `phoenix_core::domain::quota_details` and are
//! re-exported here; the `reqwest`-dependent header-parsing functions stay.

pub use phoenix_core::domain::quota_details::{
    CreditsSnapshot, QuotaDetails, QuotaLimitFamily, RateLimitReachedType, RateLimitWindow,
    SpendControlLimitSnapshot,
};
use reqwest::header::HeaderMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CodexRateLimitEventWindow {
    used_percent: f64,
    window_minutes: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimitEventDetails {
    primary: Option<CodexRateLimitEventWindow>,
    secondary: Option<CodexRateLimitEventWindow>,
    plan_type: Option<String>,
    credits: Option<CodexRateLimitEventCredits>,
    metered_limit_name: Option<String>,
    limit_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CodexRateLimitEventCredits {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimitEvent {
    #[serde(rename = "type")]
    kind: String,
    plan_type: Option<String>,
    rate_limits: Option<CodexRateLimitEventDetails>,
    credits: Option<CodexRateLimitEventCredits>,
    metered_limit_name: Option<String>,
    limit_name: Option<String>,
    rate_limit_reached_type: Option<CodexUsageReachedType>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageWindow {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageRateLimit {
    primary_window: Option<CodexUsageWindow>,
    secondary_window: Option<CodexUsageWindow>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageReachedType {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct CodexUsageIndividualLimit {
    limit: String,
    used: String,
    remaining_percent: i64,
    reset_at: i64,
}

#[derive(Debug, Deserialize)]
struct CodexUsageSpendControl {
    individual_limit: Option<CodexUsageIndividualLimit>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageAdditionalRateLimit {
    limit_name: String,
    rate_limit: CodexUsageRateLimit,
}

#[derive(Debug, Deserialize)]
struct CodexUsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<CodexUsageRateLimit>,
    credits: Option<CodexRateLimitEventCredits>,
    rate_limit_reached_type: Option<CodexUsageReachedType>,
    spend_control: Option<CodexUsageSpendControl>,
    #[serde(default)]
    additional_rate_limits: Vec<CodexUsageAdditionalRateLimit>,
}

pub fn normalize_credit_depletion(
    credits: &Option<CreditsSnapshot>,
    reached_type: Option<RateLimitReachedType>,
) -> Option<RateLimitReachedType> {
    match (credits, reached_type) {
        (
            Some(CreditsSnapshot {
                unlimited: true, ..
            }),
            Some(
                RateLimitReachedType::WorkspaceOwnerCreditsDepleted
                | RateLimitReachedType::WorkspaceMemberCreditsDepleted,
            ),
        ) => {
            tracing::warn!("ignoring contradictory Codex credit depletion for unlimited credits");
            None
        }
        (_, reached_type) => reached_type,
    }
}

/// Normalize the account-wide payload returned by Codex's authenticated usage endpoint.
#[must_use]
pub fn quota_from_codex_usage_payload(value: &serde_json::Value) -> Option<QuotaDetails> {
    let payload: CodexUsagePayload = serde_json::from_value(value.clone()).ok()?;
    let map_window = |window: CodexUsageWindow| RateLimitWindow {
        used_percent: window.used_percent,
        window_minutes: Some(window.limit_window_seconds / 60),
        resets_at: window.reset_at,
    };
    let (primary, secondary) = payload.rate_limit.map_or((None, None), |rate_limit| {
        (
            rate_limit.primary_window.map(map_window),
            rate_limit.secondary_window.map(map_window),
        )
    });
    let additional_limits = payload
        .additional_rate_limits
        .into_iter()
        .map(|family| QuotaLimitFamily {
            limit_name: family.limit_name,
            primary: family.rate_limit.primary_window.map(map_window),
            secondary: family.rate_limit.secondary_window.map(map_window),
        })
        .collect::<Vec<_>>();
    let credits = payload.credits.map(|credits| CreditsSnapshot {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance,
    });
    let individual_limit = payload
        .spend_control
        .and_then(|control| control.individual_limit)
        .map(|limit| {
            Box::new(SpendControlLimitSnapshot {
                limit: limit.limit,
                used: limit.used,
                remaining_percent: limit.remaining_percent,
                resets_at: limit.reset_at,
            })
        });
    let rate_limit_reached_type = payload
        .rate_limit_reached_type
        .and_then(|reached| parse_rate_limit_reached_type_value(&reached.kind));
    let rate_limit_reached_type = normalize_credit_depletion(&credits, rate_limit_reached_type);
    if primary.is_none()
        && secondary.is_none()
        && credits.is_none()
        && individual_limit.is_none()
        && additional_limits.is_empty()
        && rate_limit_reached_type.is_none()
        && payload.plan_type.is_none()
    {
        return None;
    }
    Some(QuotaDetails {
        plan_type: payload.plan_type,
        resets_at: None,
        limit_id: Some("codex".to_string()),
        limit_name: None,
        primary,
        secondary,
        credits,
        individual_limit,
        additional_limits,
        promo_message: None,
        rate_limit_reached_type,
    })
}

/// Normalize the complete `codex.rate_limits` WebSocket event. Its plan,
/// credits, and active limit live beside the nested window details.
#[must_use]
pub fn quota_from_codex_rate_limit_event(value: &serde_json::Value) -> Option<QuotaDetails> {
    let event: CodexRateLimitEvent = serde_json::from_value(value.clone()).ok()?;
    if event.kind != "codex.rate_limits" {
        return None;
    }
    let details = event.rate_limits;
    let (primary, secondary) = details.as_ref().map_or((None, None), |details| {
        (
            details.primary.as_ref().map(|window| RateLimitWindow {
                used_percent: window.used_percent,
                window_minutes: window.window_minutes,
                resets_at: window.reset_at,
            }),
            details.secondary.as_ref().map(|window| RateLimitWindow {
                used_percent: window.used_percent,
                window_minutes: window.window_minutes,
                resets_at: window.reset_at,
            }),
        )
    });
    let plan_type = event
        .plan_type
        .or_else(|| details.as_ref()?.plan_type.clone());
    let credits = event
        .credits
        .or_else(|| details.as_ref()?.credits.clone())
        .map(|credits| CreditsSnapshot {
            has_credits: credits.has_credits,
            unlimited: credits.unlimited,
            balance: credits.balance,
        });
    let limit_id = event
        .metered_limit_name
        .or(event.limit_name)
        .or_else(|| details.as_ref()?.metered_limit_name.clone())
        .or_else(|| details.as_ref()?.limit_name.clone())
        .map_or_else(|| "codex".to_string(), |value| canonical_limit_id(&value));
    let rate_limit_reached_type = normalize_credit_depletion(
        &credits,
        event
            .rate_limit_reached_type
            .and_then(|reached| parse_rate_limit_reached_type_value(&reached.kind)),
    );
    Some(QuotaDetails {
        plan_type,
        resets_at: None,
        limit_id: Some(limit_id),
        limit_name: None,
        primary,
        secondary,
        additional_limits: Vec::new(),
        credits,
        individual_limit: None,
        promo_message: None,
        rate_limit_reached_type,
    })
}

const ACTIVE_LIMIT_HEADER: &str = "x-codex-active-limit";
const PROMO_MESSAGE_HEADER: &str = "x-codex-promo-message";
const PLAN_TYPE_HEADER: &str = "x-codex-plan-type";
const RATE_LIMIT_REACHED_TYPE_HEADER: &str = "x-codex-rate-limit-reached-type";

/// Build a complete `QuotaDetails` snapshot from a successful codex-bridge
/// response's headers. Returns `None` only when the response carries no
/// recognized `x-codex-*` quota data at all (e.g. an account on a plan that
/// doesn't surface quota state).
pub fn quota_from_codex_response_headers(headers: &HeaderMap) -> Option<QuotaDetails> {
    // `x-codex-active-limit` is informational about the currently binding
    // tier name (e.g. "premium") — it does NOT control the prefix of the
    // window-bucket headers, which always live under `x-codex-*`. So we
    // always parse with the default prefix and use active-limit only as
    // metadata. (The named-family `parse_rate_limit_for_limit(_, Some(id))`
    // form is still used by the 429 path where the body carries an explicit
    // `limit_name` that does change the prefix.)
    let limit_id = parse_active_limit(headers);
    let (primary, secondary, limit_name) = parse_rate_limit_for_limit(headers, None);
    let credits = parse_credits_snapshot(headers);
    let plan_type = parse_header_str(headers, PLAN_TYPE_HEADER)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let promo_message = parse_promo_message(headers);

    let any_data = primary.is_some()
        || secondary.is_some()
        || credits.is_some()
        || plan_type.is_some()
        || limit_id.is_some()
        || limit_name.is_some()
        || promo_message.is_some();
    if !any_data {
        return None;
    }

    Some(QuotaDetails {
        plan_type,
        resets_at: None,
        limit_id,
        limit_name,
        primary,
        secondary,
        additional_limits: Vec::new(),
        credits,
        individual_limit: None,
        promo_message,
        rate_limit_reached_type: None,
    })
}

/// Parses the `x-codex-*` rate-limit headers for the active limit id into a
/// `(primary, secondary, limit_name)` triple. `limit_id` should match the
/// `x-codex-active-limit` header; when absent, defaults to `"codex"`.
pub fn parse_rate_limit_for_limit(
    headers: &HeaderMap,
    limit_id: Option<&str>,
) -> (
    Option<RateLimitWindow>,
    Option<RateLimitWindow>,
    Option<String>,
) {
    let normalized_limit = limit_id
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("codex")
        .to_ascii_lowercase()
        .replace('_', "-");
    let prefix = format!("x-{normalized_limit}");

    let primary = parse_rate_limit_window(
        headers,
        &format!("{prefix}-primary-used-percent"),
        &format!("{prefix}-primary-window-minutes"),
        &format!("{prefix}-primary-reset-at"),
    );
    let secondary = parse_rate_limit_window(
        headers,
        &format!("{prefix}-secondary-used-percent"),
        &format!("{prefix}-secondary-window-minutes"),
        &format!("{prefix}-secondary-reset-at"),
    );
    let limit_name = parse_header_str(headers, &format!("{prefix}-limit-name"))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    (primary, secondary, limit_name)
}

fn canonical_limit_id(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

pub fn parse_active_limit(headers: &HeaderMap) -> Option<String> {
    parse_header_str(headers, ACTIVE_LIMIT_HEADER)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(canonical_limit_id)
}

pub fn parse_promo_message(headers: &HeaderMap) -> Option<String> {
    parse_header_str(headers, PROMO_MESSAGE_HEADER)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_rate_limit_reached_type_value(value: &str) -> Option<RateLimitReachedType> {
    match value.trim() {
        "rate_limit_reached" => Some(RateLimitReachedType::RateLimitReached),
        "workspace_owner_credits_depleted" => {
            Some(RateLimitReachedType::WorkspaceOwnerCreditsDepleted)
        }
        "workspace_member_credits_depleted" => {
            Some(RateLimitReachedType::WorkspaceMemberCreditsDepleted)
        }
        "workspace_owner_usage_limit_reached" => {
            Some(RateLimitReachedType::WorkspaceOwnerUsageLimitReached)
        }
        "workspace_member_usage_limit_reached" => {
            Some(RateLimitReachedType::WorkspaceMemberUsageLimitReached)
        }
        unknown => {
            tracing::debug!(
                variant = unknown,
                "unsupported Codex rate-limit-reached type"
            );
            None
        }
    }
}

#[must_use]
pub fn parse_rate_limit_reached_type(headers: &HeaderMap) -> Option<RateLimitReachedType> {
    parse_rate_limit_reached_type_value(parse_header_str(headers, RATE_LIMIT_REACHED_TYPE_HEADER)?)
}

pub fn parse_credits_snapshot(headers: &HeaderMap) -> Option<CreditsSnapshot> {
    let has_credits = parse_header_bool(headers, "x-codex-credits-has-credits")?;
    let unlimited = parse_header_bool(headers, "x-codex-credits-unlimited")?;
    let balance = parse_header_str(headers, "x-codex-credits-balance")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    Some(CreditsSnapshot {
        has_credits,
        unlimited,
        balance,
    })
}

fn parse_rate_limit_window(
    headers: &HeaderMap,
    used_percent_header: &str,
    window_minutes_header: &str,
    resets_at_header: &str,
) -> Option<RateLimitWindow> {
    let used_percent = parse_header_f64(headers, used_percent_header)?;
    let window_minutes = parse_header_i64(headers, window_minutes_header);
    let resets_at = parse_header_i64(headers, resets_at_header);

    let has_data =
        used_percent != 0.0 || window_minutes.is_some_and(|m| m != 0) || resets_at.is_some();

    has_data.then_some(RateLimitWindow {
        used_percent,
        window_minutes,
        resets_at,
    })
}

fn parse_header_f64(headers: &HeaderMap, name: &str) -> Option<f64> {
    parse_header_str(headers, name)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn parse_header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    parse_header_str(headers, name)?.parse::<i64>().ok()
}

fn parse_header_bool(headers: &HeaderMap, name: &str) -> Option<bool> {
    let raw = parse_header_str(headers, name)?;
    if raw.eq_ignore_ascii_case("true") || raw == "1" {
        Some(true)
    } else if raw.eq_ignore_ascii_case("false") || raw == "0" {
        Some(false)
    } else {
        None
    }
}

fn parse_header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn parses_current_and_weekly_windows_from_upstream_usage_payload() {
        #![allow(clippy::float_cmp)]
        let payload = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 3,
                    "limit_window_seconds": 18_000,
                    "reset_at": 1_800_000_000
                },
                "secondary_window": {
                    "used_percent": 4,
                    "limit_window_seconds": 604_800,
                    "reset_at": 1_800_500_000
                }
            },
            "credits": { "has_credits": false, "unlimited": false, "balance": null },
            "rate_limit_reached_type": null
        });

        let quota = quota_from_codex_usage_payload(&payload).expect("usage payload");
        assert_eq!(quota.primary.as_ref().unwrap().used_percent, 3.0);
        assert_eq!(quota.primary.as_ref().unwrap().window_minutes, Some(300));
        assert_eq!(quota.secondary.as_ref().unwrap().used_percent, 4.0);
        assert_eq!(
            quota.secondary.as_ref().unwrap().window_minutes,
            Some(10_080)
        );
        assert!(!quota.credits.as_ref().unwrap().has_credits);
    }

    #[test]
    fn preserves_additional_rate_limit_families() {
        let payload = serde_json::json!({
            "additional_rate_limits": [{
                "limit_name": "review",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 12,
                        "limit_window_seconds": 18_000,
                        "reset_at": 1_800_000_000
                    },
                    "secondary_window": null
                }
            }]
        });

        let quota = quota_from_codex_usage_payload(&payload).expect("additional quota family");
        assert_eq!(quota.additional_limits.len(), 1);
        assert_eq!(quota.additional_limits[0].limit_name, "review");
        assert_eq!(
            quota.additional_limits[0]
                .primary
                .as_ref()
                .unwrap()
                .window_minutes,
            Some(300)
        );
    }

    #[test]
    fn websocket_event_preserves_explicit_depletion_reason() {
        let event = serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": null,
            "credits": null,
            "rate_limit_reached_type": {
                "type": "workspace_owner_credits_depleted"
            }
        });
        let quota = quota_from_codex_rate_limit_event(&event).expect("websocket quota");
        assert_eq!(
            quota.rate_limit_reached_type,
            Some(RateLimitReachedType::WorkspaceOwnerCreditsDepleted)
        );
    }

    #[test]
    fn websocket_event_rejects_depletion_for_unlimited_credits() {
        let event = serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": null,
            "credits": { "has_credits": true, "unlimited": true, "balance": null },
            "rate_limit_reached_type": {
                "type": "workspace_owner_credits_depleted"
            }
        });
        let quota = quota_from_codex_rate_limit_event(&event).expect("websocket quota");
        assert!(quota.rate_limit_reached_type.is_none());
    }

    #[test]
    fn preserves_spend_control_without_standard_rate_windows() {
        let payload = serde_json::json!({
            "spend_control": {
                "individual_limit": {
                    "limit": "100.00",
                    "used": "25.00",
                    "remaining_percent": 75,
                    "reset_at": 1_800_000_000
                }
            }
        });

        let quota = quota_from_codex_usage_payload(&payload).expect("spend-control payload");
        let limit = quota.individual_limit.expect("individual limit");
        assert_eq!(limit.limit, "100.00");
        assert_eq!(limit.used, "25.00");
        assert_eq!(limit.remaining_percent, 75);
        assert_eq!(limit.resets_at, 1_800_000_000);
    }

    #[test]
    fn preserves_credit_only_usage_payload() {
        let payload = serde_json::json!({
            "plan_type": null,
            "rate_limit": null,
            "credits": { "has_credits": false, "unlimited": false, "balance": null },
            "rate_limit_reached_type": {
                "type": "workspace_member_credits_depleted"
            }
        });

        let quota = quota_from_codex_usage_payload(&payload).expect("credit-only usage payload");
        assert!(quota.primary.is_none());
        assert!(quota.secondary.is_none());
        assert!(!quota.credits.as_ref().unwrap().unlimited);
        assert_eq!(
            quota.rate_limit_reached_type,
            Some(RateLimitReachedType::WorkspaceMemberCreditsDepleted)
        );
    }

    #[test]
    fn rejects_credit_depletion_that_contradicts_unlimited_credits() {
        let payload = serde_json::json!({
            "credits": { "has_credits": true, "unlimited": true, "balance": null },
            "rate_limit_reached_type": {
                "type": "workspace_member_credits_depleted"
            }
        });
        let quota = quota_from_codex_usage_payload(&payload).expect("credits payload");
        assert!(quota.rate_limit_reached_type.is_none());
        assert!(quota.credits.unwrap().unlimited);
    }

    #[test]
    fn rejects_usage_payload_without_quota_data() {
        assert!(quota_from_codex_usage_payload(&serde_json::json!({})).is_none());
    }

    #[test]
    // Parsed percentages are exact, representable values from the fixture header.
    #[allow(clippy::float_cmp)]
    fn parses_default_codex_primary_window() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("12.5"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("60"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("1704069000"),
        );

        let (primary, secondary, name) = parse_rate_limit_for_limit(&headers, None);
        let primary = primary.expect("primary");
        assert_eq!(primary.used_percent, 12.5);
        assert_eq!(primary.window_minutes, Some(60));
        assert_eq!(primary.resets_at, Some(1_704_069_000));
        assert!(secondary.is_none());
        assert!(name.is_none());
    }

    #[test]
    fn parses_explicit_rate_limit_reached_types() {
        for (raw, expected) in [
            (
                "workspace_owner_credits_depleted",
                RateLimitReachedType::WorkspaceOwnerCreditsDepleted,
            ),
            (
                "workspace_member_credits_depleted",
                RateLimitReachedType::WorkspaceMemberCreditsDepleted,
            ),
            (
                "workspace_owner_usage_limit_reached",
                RateLimitReachedType::WorkspaceOwnerUsageLimitReached,
            ),
            (
                "workspace_member_usage_limit_reached",
                RateLimitReachedType::WorkspaceMemberUsageLimitReached,
            ),
            ("rate_limit_reached", RateLimitReachedType::RateLimitReached),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                RATE_LIMIT_REACHED_TYPE_HEADER,
                HeaderValue::from_str(raw).expect("header value"),
            );
            assert_eq!(parse_rate_limit_reached_type(&headers), Some(expected));
        }
    }

    #[test]
    fn ignores_unknown_rate_limit_reached_type() {
        let mut headers = HeaderMap::new();
        headers.insert(
            RATE_LIMIT_REACHED_TYPE_HEADER,
            HeaderValue::from_static("future_limit_type"),
        );
        assert_eq!(parse_rate_limit_reached_type(&headers), None);
    }

    #[test]
    fn canonicalizes_active_limit_family() {
        let mut headers = HeaderMap::new();
        headers.insert(ACTIVE_LIMIT_HEADER, HeaderValue::from_static("CODEX_OTHER"));
        assert_eq!(parse_active_limit(&headers).as_deref(), Some("codex-other"));
    }

    #[test]
    fn parses_credits_snapshot() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-credits-has-credits",
            HeaderValue::from_static("true"),
        );
        headers.insert(
            "x-codex-credits-unlimited",
            HeaderValue::from_static("false"),
        );
        headers.insert("x-codex-credits-balance", HeaderValue::from_static("$3.42"));

        let credits = parse_credits_snapshot(&headers).expect("credits");
        assert!(credits.has_credits);
        assert!(!credits.unlimited);
        assert_eq!(credits.balance.as_deref(), Some("$3.42"));
    }

    #[test]
    fn parses_limit_name_for_named_family() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-other-primary-used-percent",
            HeaderValue::from_static("80"),
        );
        headers.insert(
            "x-codex-other-limit-name",
            HeaderValue::from_static("gpt-5.2-codex-sonic"),
        );

        let (primary, _, name) = parse_rate_limit_for_limit(&headers, Some("codex_other"));
        assert!(primary.is_some());
        assert_eq!(name.as_deref(), Some("gpt-5.2-codex-sonic"));
    }

    #[test]
    fn parses_promo_message_and_trims() {
        let mut headers = HeaderMap::new();
        headers.insert(
            PROMO_MESSAGE_HEADER,
            HeaderValue::from_static("  Upgrade to Pro  "),
        );
        assert_eq!(
            parse_promo_message(&headers).as_deref(),
            Some("Upgrade to Pro")
        );
    }

    #[test]
    // Parsed percentages are exact, representable values from the fixture.
    #[allow(clippy::float_cmp)]
    fn full_websocket_event_preserves_top_level_metadata_and_reset_at() {
        let quota = quota_from_codex_rate_limit_event(&serde_json::json!({
            "type": "codex.rate_limits",
            "plan_type": "pro",
            "metered_limit_name": "codex_other",
            "rate_limits": {
                "primary": {"used_percent": 75.5, "window_minutes": 300, "reset_at": 1_738_888_888},
                "secondary": {"used_percent": 20.0, "window_minutes": 10_080, "reset_at": 1_739_999_999}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "42.5"}
        }))
        .expect("rate-limit event");
        assert_eq!(quota.plan_type.as_deref(), Some("pro"));
        assert_eq!(quota.limit_id.as_deref(), Some("codex-other"));
        assert_eq!(
            quota.primary.as_ref().and_then(|w| w.resets_at),
            Some(1_738_888_888)
        );
        assert_eq!(
            quota.secondary.as_ref().and_then(|w| w.resets_at),
            Some(1_739_999_999)
        );
        let credits = quota.credits.expect("credits");
        assert!(credits.has_credits);
        assert!(!credits.unlimited);
        assert_eq!(credits.balance.as_deref(), Some("42.5"));
    }

    #[test]
    // Parsed percentages are exact, representable values from the fixture.
    #[allow(clippy::float_cmp)]
    fn nested_websocket_event_preserves_metadata() {
        let quota = quota_from_codex_rate_limit_event(&serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": {
                "plan_type": "plus",
                "metered_limit_name": "codex_mini",
                "primary": {"used_percent": 90.0, "window_minutes": 300, "reset_at": 1_738_888_888},
                "credits": {"has_credits": true, "unlimited": false, "balance": "7"}
            }
        }))
        .expect("nested event");
        assert_eq!(quota.plan_type.as_deref(), Some("plus"));
        assert_eq!(quota.limit_id.as_deref(), Some("codex-mini"));
        assert_eq!(quota.primary.as_ref().map(|w| w.used_percent), Some(90.0));
        assert_eq!(
            quota.primary.as_ref().and_then(|w| w.resets_at),
            Some(1_738_888_888)
        );
        assert_eq!(quota.credits.and_then(|c| c.balance).as_deref(), Some("7"));
    }

    #[test]
    // Parsed percentages are exact, representable values from the fixture header.
    #[allow(clippy::float_cmp)]
    fn quota_from_response_headers_full() {
        let mut headers = HeaderMap::new();
        headers.insert(PLAN_TYPE_HEADER, HeaderValue::from_static("plus"));
        headers.insert(ACTIVE_LIMIT_HEADER, HeaderValue::from_static("premium"));
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("12.5"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("1779756466"),
        );
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("48"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("10080"),
        );
        headers.insert(
            "x-codex-credits-has-credits",
            HeaderValue::from_static("false"),
        );
        headers.insert(
            "x-codex-credits-unlimited",
            HeaderValue::from_static("false"),
        );

        let q = quota_from_codex_response_headers(&headers).expect("snapshot");
        assert_eq!(q.plan_type.as_deref(), Some("plus"));
        assert_eq!(q.limit_id.as_deref(), Some("premium"));
        let p = q.primary.expect("primary");
        assert_eq!(p.used_percent, 12.5);
        assert_eq!(p.window_minutes, Some(300));
        assert_eq!(p.resets_at, Some(1_779_756_466));
        let s = q.secondary.expect("secondary");
        assert_eq!(s.used_percent, 48.0);
        assert_eq!(s.window_minutes, Some(10080));
        let c = q.credits.expect("credits");
        assert!(!c.has_credits);
        assert!(!c.unlimited);
    }

    #[test]
    fn quota_from_response_headers_empty_returns_none() {
        let headers = HeaderMap::new();
        assert!(quota_from_codex_response_headers(&headers).is_none());
    }

    #[test]
    fn quota_from_response_headers_plan_type_alone_returns_some() {
        let mut headers = HeaderMap::new();
        headers.insert(PLAN_TYPE_HEADER, HeaderValue::from_static("free"));
        let q = quota_from_codex_response_headers(&headers).expect("snapshot");
        assert_eq!(q.plan_type.as_deref(), Some("free"));
        assert!(q.primary.is_none());
    }
}
