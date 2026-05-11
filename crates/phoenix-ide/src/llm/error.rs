//! LLM error types

use super::rate_limit::QuotaDetails;
use chrono::{DateTime, Datelike, Local, Utc};
use thiserror::Error;

/// LLM error with classification
#[derive(Debug, Error)]
#[error("{message}")]
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    /// When true, a recovery mechanism (e.g. credential helper) is actively
    /// running and may resolve this error. The state machine should wait
    /// rather than treat it as terminal.
    pub recovery_in_progress: bool,
    /// Present iff `kind == UsageLimitReached`. Structured payload extracted
    /// from the codex backend's 429 response (body + headers). Used to render
    /// plan-aware messages and (later) drive a quota status indicator. Boxed
    /// because `UsageLimitReached` is the rare path and this keeps `LlmError`
    /// small enough that `Result<_, LlmError>` stays under clippy's
    /// `result_large_err` threshold across the LLM hot path.
    pub quota: Option<Box<QuotaDetails>>,
}

impl LlmError {
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            recovery_in_progress: false,
            quota: None,
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::Network, message)
    }

    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::RateLimit, message)
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ServerError, message)
    }

    pub fn server_overloaded(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ServerOverloaded, message)
    }

    pub fn usage_limit_reached(quota: QuotaDetails) -> Self {
        let message = render_usage_limit_message(&quota);
        Self {
            kind: LlmErrorKind::UsageLimitReached,
            message,
            recovery_in_progress: false,
            quota: Some(Box::new(quota)),
        }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::Auth, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::InvalidRequest, message)
    }

    #[allow(dead_code)] // Will be used when providers detect content filter responses
    pub fn content_filter(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ContentFilter, message)
    }

    #[allow(dead_code)] // Will be used when providers detect context window errors
    pub fn context_window_exceeded(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::ContextWindowExceeded, message)
    }
}

/// Error classification for retry logic.
///
/// No `Unknown` variant. No `#[non_exhaustive]`. Adding a new error class
/// requires adding a variant here and handling it in every consumer — the
/// compiler forces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Network issues, timeouts - retryable
    Network,
    /// Transient rate-limit throttle (per-minute, per-second windows) - retryable with backoff
    RateLimit,
    /// Quota window exhausted (plan-level cap hit, credits depleted, etc.) - NOT retryable
    UsageLimitReached,
    /// Server error (5xx) - retryable
    ServerError,
    /// Selected model is at capacity (`server_is_overloaded` / `slow_down`) - NOT retryable
    ServerOverloaded,
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Content filter or safety block - not retryable
    #[allow(dead_code)] // Will be used when providers detect content filter responses
    ContentFilter,
    /// Context window exceeded - not retryable in current conversation
    #[allow(dead_code)] // Will be used when providers detect context window errors
    ContextWindowExceeded,
}

impl LlmErrorKind {
    pub fn is_retryable(self) -> bool {
        match self {
            Self::Network | Self::RateLimit | Self::ServerError => true,
            Self::UsageLimitReached
            | Self::ServerOverloaded
            | Self::Auth
            | Self::InvalidRequest
            | Self::ContentFilter
            | Self::ContextWindowExceeded => false,
        }
    }
}

impl LlmError {
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(LlmErrorKind::InvalidRequest, message)
    }

    pub fn from_http_status(status: u16, body: &str) -> Self {
        match status {
            401 | 403 => Self::auth(format!("Authentication failed: {body}")),
            429 => Self::rate_limit(format!("Rate limited: {body}")),
            400..=499 => Self::invalid_request(format!("Bad request ({status}): {body}")),
            500..=599 => Self::server_error(format!("Server error ({status}): {body}")),
            // Unexpected status (1xx, 3xx, etc.) — treat as retryable server error
            _ => Self::server_error(format!("Unexpected HTTP {status}: {body}")),
        }
    }
}

/// Render a plan-aware "usage limit reached" message for the codex backend.
///
/// Wording mirrors the codex CLI's `UsageLimitReachedError::fmt`
/// (`/tmp/codex/codex-rs/protocol/src/error.rs:453-517`) verbatim so users see
/// the same recovery instructions across tools.
fn render_usage_limit_message(quota: &QuotaDetails) -> String {
    // 1. Per-model limit override: when `limit_name` is set and isn't the
    //    generic "codex" family, the user can switch models to keep working.
    if let Some(limit_name) = quota
        .limit_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if !limit_name.eq_ignore_ascii_case("codex") {
            return format!(
                "You've hit your usage limit for {limit_name}. Switch to another model now,{}",
                retry_suffix_after_or(quota.resets_at.as_ref())
            );
        }
    }

    // 2. Backend-provided promo message wins over plan-specific defaults.
    if let Some(promo) = quota.promo_message.as_deref() {
        return format!(
            "You've hit your usage limit. {promo},{}",
            retry_suffix_after_or(quota.resets_at.as_ref())
        );
    }

    // 3. Plan-aware defaults. Plan-type strings are matched case-insensitively
    //    against the values the codex backend sends.
    let plan = quota.plan_type.as_deref().map(str::to_ascii_lowercase);
    match plan.as_deref() {
        Some("plus") => format!(
            "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits{}",
            retry_suffix_after_or(quota.resets_at.as_ref())
        ),
        Some(
            "team"
            | "business"
            | "self_serve_business_usage_based"
            | "enterprise_cbp_usage_based",
        ) => format!(
            "You've hit your usage limit. To get more access now, send a request to your admin{}",
            retry_suffix_after_or(quota.resets_at.as_ref())
        ),
        Some("free" | "go") => format!(
            "You've hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus),{}",
            retry_suffix_after_or(quota.resets_at.as_ref())
        ),
        Some("pro" | "pro_lite") => format!(
            "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits{}",
            retry_suffix_after_or(quota.resets_at.as_ref())
        ),
        // Enterprise / Edu and unknown / absent plans all collapse to the
        // generic wording. Codex CLI keeps these as separate match arms for
        // documentation; we merge to satisfy clippy::match_same_arms.
        _ => format!(
            "You've hit your usage limit.{}",
            retry_suffix(quota.resets_at.as_ref())
        ),
    }
}

fn retry_suffix(resets_at: Option<&DateTime<Utc>>) -> String {
    match resets_at {
        Some(ts) => format!(" Try again at {}.", format_retry_timestamp(ts)),
        None => " Try again later.".to_string(),
    }
}

fn retry_suffix_after_or(resets_at: Option<&DateTime<Utc>>) -> String {
    match resets_at {
        Some(ts) => format!(" or try again at {}.", format_retry_timestamp(ts)),
        None => " or try again later.".to_string(),
    }
}

fn format_retry_timestamp(resets_at: &DateTime<Utc>) -> String {
    let local_reset = resets_at.with_timezone(&Local);
    let local_now = now_for_retry().with_timezone(&Local);
    if local_reset.date_naive() == local_now.date_naive() {
        local_reset.format("%-I:%M %p").to_string()
    } else {
        let suffix = day_suffix(local_reset.day());
        local_reset
            .format(&format!("%b %-d{suffix}, %Y %-I:%M %p"))
            .to_string()
    }
}

fn day_suffix(day: u32) -> &'static str {
    match day {
        11..=13 => "th",
        _ => match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    }
}

#[cfg(test)]
thread_local! {
    static NOW_OVERRIDE: std::cell::RefCell<Option<DateTime<Utc>>> =
        const { std::cell::RefCell::new(None) };
}

fn now_for_retry() -> DateTime<Utc> {
    #[cfg(test)]
    {
        if let Some(now) = NOW_OVERRIDE.with(|cell| *cell.borrow()) {
            return now;
        }
    }
    Utc::now()
}

#[cfg(test)]
pub(crate) fn set_now_override(now: Option<DateTime<Utc>>) {
    NOW_OVERRIDE.with(|cell| *cell.borrow_mut() = now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::rate_limit::QuotaDetails;
    use chrono::TimeZone;

    fn quota(plan: Option<&str>, resets_at: Option<DateTime<Utc>>) -> QuotaDetails {
        QuotaDetails {
            plan_type: plan.map(str::to_string),
            resets_at,
            limit_id: None,
            limit_name: None,
            primary: None,
            secondary: None,
            credits: None,
            promo_message: None,
        }
    }

    /// Pin "now" to a fixed UTC moment so `format_retry_timestamp`'s
    /// same-day-vs-cross-day branch is deterministic regardless of when tests run.
    fn with_fixed_now<F: FnOnce()>(now: DateTime<Utc>, f: F) {
        set_now_override(Some(now));
        f();
        set_now_override(None);
    }

    #[test]
    fn plus_plan_renders_upgrade_path() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        let resets = Utc.with_ymd_and_hms(2026, 5, 11, 23, 42, 0).unwrap();
        with_fixed_now(now, || {
            let msg = render_usage_limit_message(&quota(Some("plus"), Some(resets)));
            assert!(
                msg.contains("Upgrade to Pro"),
                "expected upgrade-to-pro wording, got: {msg}"
            );
            assert!(msg.contains("or try again at"), "got: {msg}");
        });
    }

    #[test]
    fn team_plan_renders_admin_path() {
        let msg = render_usage_limit_message(&quota(Some("team"), None));
        assert!(msg.contains("send a request to your admin"), "got: {msg}");
        assert!(msg.contains("or try again later."));
    }

    #[test]
    fn pro_plan_renders_credits_path() {
        let msg = render_usage_limit_message(&quota(Some("pro"), None));
        assert!(
            msg.contains("purchase more credits"),
            "expected credits wording, got: {msg}"
        );
    }

    #[test]
    fn free_plan_renders_plus_upgrade() {
        let msg = render_usage_limit_message(&quota(Some("free"), None));
        assert!(msg.contains("Upgrade to Plus"), "got: {msg}");
    }

    #[test]
    fn enterprise_plan_omits_recovery_action() {
        let msg = render_usage_limit_message(&quota(Some("enterprise"), None));
        assert_eq!(msg, "You've hit your usage limit. Try again later.");
    }

    #[test]
    fn unknown_plan_falls_back_to_generic() {
        let msg = render_usage_limit_message(&quota(Some("mysteryplan"), None));
        assert_eq!(msg, "You've hit your usage limit. Try again later.");
    }

    #[test]
    fn none_plan_falls_back_to_generic() {
        let msg = render_usage_limit_message(&quota(None, None));
        assert_eq!(msg, "You've hit your usage limit. Try again later.");
    }

    #[test]
    fn promo_message_overrides_plan_wording() {
        let mut q = quota(Some("plus"), None);
        q.promo_message = Some("Upgrade to Pro at chatgpt.com/explore/pro".to_string());
        let msg = render_usage_limit_message(&q);
        assert!(msg.starts_with("You've hit your usage limit. Upgrade to Pro at"));
        assert!(msg.contains(", or try again later."));
    }

    #[test]
    fn limit_name_other_than_codex_suggests_switching_models() {
        let mut q = quota(Some("plus"), None);
        q.limit_name = Some("gpt-5.2-codex-sonic".to_string());
        let msg = render_usage_limit_message(&q);
        assert!(
            msg.starts_with(
                "You've hit your usage limit for gpt-5.2-codex-sonic. Switch to another model now,"
            ),
            "got: {msg}"
        );
    }

    #[test]
    fn limit_name_equal_to_codex_falls_through_to_plan_branch() {
        let mut q = quota(Some("plus"), None);
        q.limit_name = Some("codex".to_string());
        let msg = render_usage_limit_message(&q);
        assert!(msg.contains("Upgrade to Pro"), "got: {msg}");
    }

    #[test]
    fn same_day_reset_renders_time_only() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        let resets = Utc.with_ymd_and_hms(2026, 5, 11, 23, 42, 0).unwrap();
        with_fixed_now(now, || {
            let formatted = format_retry_timestamp(&resets);
            // Local timezone may shift the rendered hour; verify shape only.
            assert!(
                formatted.contains(":42 PM") || formatted.contains(":42 AM"),
                "expected HH:MM AM/PM, got {formatted}"
            );
        });
    }

    #[test]
    fn day_suffix_matches_codex_cli() {
        assert_eq!(day_suffix(1), "st");
        assert_eq!(day_suffix(2), "nd");
        assert_eq!(day_suffix(3), "rd");
        assert_eq!(day_suffix(4), "th");
        assert_eq!(day_suffix(11), "th");
        assert_eq!(day_suffix(12), "th");
        assert_eq!(day_suffix(13), "th");
        assert_eq!(day_suffix(21), "st");
        assert_eq!(day_suffix(22), "nd");
        assert_eq!(day_suffix(23), "rd");
    }

    #[test]
    fn usage_limit_reached_is_not_retryable() {
        assert!(!LlmErrorKind::UsageLimitReached.is_retryable());
        assert!(!LlmErrorKind::ServerOverloaded.is_retryable());
        assert!(LlmErrorKind::RateLimit.is_retryable());
    }
}
