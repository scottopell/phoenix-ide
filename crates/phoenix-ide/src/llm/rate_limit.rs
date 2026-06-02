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
//! Phoenix uses the HTTP/SSE transport against this backend. The WebSocket
//! variant of the same endpoint emits a richer mid-stream `codex.rate_limits`
//! frame (consumed by codex CLI) — the HTTP path never sees that frame, so
//! the response headers are the single source of truth here.
//!
//! These types intentionally mirror the codex CLI's `RateLimitSnapshot` shape
//! (`codex-rs/protocol/src/protocol.rs`) without depending on the
//! `codex_protocol` / `codex_api` crates — we only need the data layout.
//!
//! The pure-data snapshot types (`QuotaDetails`, `RateLimitWindow`,
//! `CreditsSnapshot`) live in `phoenix_core::domain::quota_details` and are
//! re-exported here; the `reqwest`-dependent header-parsing functions stay.

pub use phoenix_core::domain::quota_details::{CreditsSnapshot, QuotaDetails, RateLimitWindow};
use reqwest::header::HeaderMap;

const ACTIVE_LIMIT_HEADER: &str = "x-codex-active-limit";
const PROMO_MESSAGE_HEADER: &str = "x-codex-promo-message";
const PLAN_TYPE_HEADER: &str = "x-codex-plan-type";

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
        credits,
        promo_message,
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

    #[test]
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
        assert_eq!(p.resets_at, Some(1779756466));
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
