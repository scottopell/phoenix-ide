//! Wire types for Codex rate-limit reset credits (specs/codex-rate-limit-resets).
//!
//! The upstream decoding lives in `phoenix_llm::codex_reset_credits`; these are
//! the shapes Phoenix's own API returns to the UI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum CodexResetType {
    CodexRateLimits,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum CodexResetCreditStatus {
    Available,
    Redeeming,
    Redeemed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CodexResetCredit {
    pub id: CodexResetCreditId,
    pub reset_type: CodexResetType,
    pub status: CodexResetCreditStatus,
    /// Unix seconds.
    pub granted_at: i64,
    /// Unix seconds; `None` means the credit does not expire.
    pub expires_at: Option<i64>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CodexResetCredits {
    /// `ChatGPT` account the listing was read for; a redemption must name it.
    pub account_id: Option<String>,
    pub available_count: u32,
    pub credits: Vec<CodexResetCredit>,
}

/// Upstream reset-credit identifier; empty identifiers are unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/", type = "string")]
pub struct CodexResetCreditId(String);

impl Serialize for CodexResetCreditId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CodexResetCreditId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl CodexResetCreditId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CodexResetCreditId {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            Err("credit_id must not be empty")
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CodexResetRedeemRequest {
    #[ts(type = "string")]
    pub idempotency_key: uuid::Uuid,
    /// `None` lets the backend choose the credit.
    pub credit_id: Option<CodexResetCreditId>,
    /// The `account_id` of the listing the user chose from.
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum CodexResetRedeemOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

impl CodexResetRedeemOutcome {
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Reset | Self::AlreadyRedeemed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CodexResetRedeemResponse {
    pub outcome: CodexResetRedeemOutcome,
    pub windows_reset: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redeem_request_rejects_empty_credit_id() {
        let key = "00000000-0000-0000-0000-000000000000";
        for credit in ["\"\"", "\"  \""] {
            let body =
                format!(r#"{{"idempotency_key":"{key}","credit_id":{credit},"account_id":null}}"#);
            assert!(serde_json::from_str::<CodexResetRedeemRequest>(&body).is_err());
        }
        let body = format!(r#"{{"idempotency_key":"{key}","credit_id":"c1","account_id":"acct"}}"#);
        let request: CodexResetRedeemRequest = serde_json::from_str(&body).unwrap();
        assert_eq!(request.credit_id.unwrap().as_str(), "c1");
    }

    #[test]
    fn redeem_request_rejects_non_uuid_key() {
        let body = r#"{"idempotency_key":"retry-1","credit_id":null,"account_id":null}"#;
        assert!(serde_json::from_str::<CodexResetRedeemRequest>(body).is_err());
    }
}
