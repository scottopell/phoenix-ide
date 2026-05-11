//! Structured quota and rate-limit state extracted from the codex backend.
//!
//! The codex backend (`chatgpt.com/backend-api/codex`) returns plan and quota
//! state in both the 429 response body (`plan_type`, `resets_at`) and the
//! `x-codex-*` response headers (window snapshots, credits, promo). Phoenix
//! parses both to render plan-aware terminal errors.
//!
//! These types intentionally mirror the codex CLI's `RateLimitSnapshot` shape
//! (`codex-rs/protocol/src/protocol.rs`) without depending on the
//! `codex_protocol` / `codex_api` crates — we only need the data layout.

use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;

/// Structured quota state extracted from the codex backend on 429.
///
/// All fields are optional: the codex backend populates a subset depending on
/// which limit was hit (per-model vs global), the user's plan, and whether
/// credits are tracked. Consumers must handle every field being `None`.
#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

const ACTIVE_LIMIT_HEADER: &str = "x-codex-active-limit";
const PROMO_MESSAGE_HEADER: &str = "x-codex-promo-message";

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

pub fn parse_active_limit(headers: &HeaderMap) -> Option<String> {
    parse_header_str(headers, ACTIVE_LIMIT_HEADER)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

pub fn parse_promo_message(headers: &HeaderMap) -> Option<String> {
    parse_header_str(headers, PROMO_MESSAGE_HEADER)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
        assert_eq!(primary.resets_at, Some(1704069000));
        assert!(secondary.is_none());
        assert!(name.is_none());
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
}
