//! Upstream contract for Codex rate-limit reset credits
//! (specs/codex-rate-limit-resets). Mirrors `codex-rs/backend-client`
//! `Client::list_rate_limit_reset_credits` and
//! `Client::consume_rate_limit_reset_credit_request`.

use chrono::DateTime;
use phoenix_core::domain::codex_reset_credits::{
    CodexResetCredit, CodexResetCreditStatus, CodexResetCredits, CodexResetRedeemOutcome,
    CodexResetRedeemResponse, CodexResetType,
};
use serde::{Deserialize, Serialize};

pub const CODEX_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
pub const CODEX_RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";

#[derive(Debug, Deserialize)]
struct UpstreamResetCredit {
    id: String,
    reset_type: String,
    status: String,
    granted_at: String,
    expires_at: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpstreamResetCredits {
    credits: Vec<UpstreamResetCredit>,
    available_count: i64,
}

#[derive(Debug, Deserialize)]
struct UpstreamRedeemResponse {
    code: CodexResetRedeemOutcome,
    // owned: upstream omits windows_reset on non-reset outcomes; zero is correct.
    #[serde(default)]
    windows_reset: i64,
}

/// JSON body for the upstream consume request.
#[derive(Debug, Serialize)]
pub struct UpstreamRedeemRequest<'a> {
    redeem_request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit_id: Option<&'a str>,
}

impl<'a> UpstreamRedeemRequest<'a> {
    #[must_use]
    pub fn new(idempotency_key: uuid::Uuid, credit_id: Option<&'a str>) -> Self {
        Self {
            redeem_request_id: idempotency_key.to_string(),
            credit_id,
        }
    }
}

fn unix_seconds(field: &str, credit_id: &str, value: &str) -> Result<i64, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.timestamp())
        .map_err(|error| {
            format!("invalid {field} `{value}` for reset credit `{credit_id}`: {error}")
        })
}

fn reset_type(value: &str) -> CodexResetType {
    match value {
        "codex_rate_limits" => CodexResetType::CodexRateLimits,
        _ => CodexResetType::Unknown,
    }
}

fn credit_status(value: &str) -> CodexResetCreditStatus {
    match value {
        "available" => CodexResetCreditStatus::Available,
        "redeeming" => CodexResetCreditStatus::Redeeming,
        "redeemed" => CodexResetCreditStatus::Redeemed,
        _ => CodexResetCreditStatus::Unknown,
    }
}

/// Decode the upstream listing read for `account_id`. Any malformed credit fails the whole listing
/// (REQ-CRR-001): a fabricated time would be worse than no listing.
///
/// # Errors
///
/// Returns a description when the body does not decode or a time is not RFC3339.
pub fn reset_credits_from_upstream(
    value: serde_json::Value,
    account_id: Option<String>,
) -> Result<CodexResetCredits, String> {
    let upstream: UpstreamResetCredits =
        serde_json::from_value(value).map_err(|error| format!("reset credit listing: {error}"))?;
    let credits = upstream
        .credits
        .into_iter()
        .map(|credit| {
            let granted_at = unix_seconds("granted_at", &credit.id, &credit.granted_at)?;
            let expires_at = credit
                .expires_at
                .as_deref()
                .map(|value| unix_seconds("expires_at", &credit.id, value))
                .transpose()?;
            Ok(CodexResetCredit {
                reset_type: reset_type(&credit.reset_type),
                status: credit_status(&credit.status),
                granted_at,
                expires_at,
                title: credit.title,
                description: credit.description,
                id: credit.id,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(CodexResetCredits {
        account_id,
        available_count: u32::try_from(upstream.available_count.max(0)).unwrap_or(u32::MAX),
        credits,
    })
}

/// Decode the upstream consume answer. An unknown `code` is an error, never a
/// success (REQ-CRR-002).
///
/// # Errors
///
/// Returns a description when the body does not decode or the code is unknown.
pub fn redeem_response_from_upstream(
    value: serde_json::Value,
) -> Result<CodexResetRedeemResponse, String> {
    let upstream: UpstreamRedeemResponse =
        serde_json::from_value(value).map_err(|error| format!("reset redeem response: {error}"))?;
    Ok(CodexResetRedeemResponse {
        outcome: upstream.code,
        windows_reset: upstream.windows_reset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn listing_decodes_upstream_fixture() {
        // Same payload shape as codex-rs rate_limit_reset_contract tests.
        let listing = reset_credits_from_upstream(
            json!({
                "credits": [
                    {
                        "id": "credit-1",
                        "reset_type": "codex_rate_limits",
                        "status": "available",
                        "granted_at": "2026-06-17T00:00:00Z",
                        "expires_at": "2026-07-17T00:00:00Z",
                        "redeem_started_at": null,
                        "redeemed_at": null,
                        "profile_user_id": "@friend",
                        "title": "Full reset (Weekly + 5 hr)",
                        "description": "Ready to redeem"
                    },
                    {
                        "id": "credit-2",
                        "reset_type": "something_new",
                        "status": "frozen",
                        "granted_at": "2026-06-18T00:00:00+02:00",
                        "expires_at": null
                    }
                ],
                "available_count": 2,
                "total_earned_count": 4
            }),
            Some("acct".into()),
        )
        .unwrap();
        assert_eq!(
            listing,
            CodexResetCredits {
                account_id: Some("acct".into()),
                available_count: 2,
                credits: vec![
                    CodexResetCredit {
                        id: "credit-1".into(),
                        reset_type: CodexResetType::CodexRateLimits,
                        status: CodexResetCreditStatus::Available,
                        granted_at: 1_781_654_400,
                        expires_at: Some(1_784_246_400),
                        title: Some("Full reset (Weekly + 5 hr)".into()),
                        description: Some("Ready to redeem".into()),
                    },
                    CodexResetCredit {
                        id: "credit-2".into(),
                        reset_type: CodexResetType::Unknown,
                        status: CodexResetCreditStatus::Unknown,
                        granted_at: 1_781_733_600,
                        expires_at: None,
                        title: None,
                        description: None,
                    },
                ],
            }
        );
    }

    #[test]
    fn listing_rejects_malformed_time() {
        let error = reset_credits_from_upstream(
            json!({
                "credits": [{
                    "id": "c", "reset_type": "codex_rate_limits", "status": "available",
                    "granted_at": "yesterday", "expires_at": null
                }],
                "available_count": 1
            }),
            None,
        )
        .unwrap_err();
        assert!(error.contains("granted_at"), "{error}");
    }

    #[test]
    fn listing_requires_credits_and_count() {
        assert!(reset_credits_from_upstream(json!({ "available_count": 1 }), None).is_err());
        assert!(reset_credits_from_upstream(json!({ "credits": [] }), None).is_err());
    }

    #[test]
    fn listing_clamps_negative_count() {
        let listing =
            reset_credits_from_upstream(json!({ "credits": [], "available_count": -3 }), None)
                .unwrap();
        assert_eq!(listing.available_count, 0);
    }

    #[test]
    fn redeem_decodes_every_known_code() {
        for (code, outcome) in [
            ("reset", CodexResetRedeemOutcome::Reset),
            ("nothing_to_reset", CodexResetRedeemOutcome::NothingToReset),
            ("no_credit", CodexResetRedeemOutcome::NoCredit),
            ("already_redeemed", CodexResetRedeemOutcome::AlreadyRedeemed),
        ] {
            let response = redeem_response_from_upstream(json!({ "code": code })).unwrap();
            assert_eq!(response.outcome, outcome);
            assert_eq!(response.windows_reset, 0);
        }
        let response = redeem_response_from_upstream(json!({
            "code": "reset", "credit": { "id": "ignored" }, "windows_reset": 2
        }))
        .unwrap();
        assert_eq!(response.windows_reset, 2);
        assert!(response.outcome.is_success());
    }

    #[test]
    fn redeem_rejects_unknown_code() {
        assert!(redeem_response_from_upstream(json!({ "code": "partially_reset" })).is_err());
    }

    #[test]
    fn redeem_request_matches_upstream_body() {
        let key = uuid::Uuid::nil();
        assert_eq!(
            serde_json::to_value(UpstreamRedeemRequest::new(key, None)).unwrap(),
            json!({ "redeem_request_id": "00000000-0000-0000-0000-000000000000" })
        );
        assert_eq!(
            serde_json::to_value(UpstreamRedeemRequest::new(key, Some("credit-1"))).unwrap(),
            json!({
                "redeem_request_id": "00000000-0000-0000-0000-000000000000",
                "credit_id": "credit-1"
            })
        );
    }
}
