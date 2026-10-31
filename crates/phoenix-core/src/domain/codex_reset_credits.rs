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
    pub id: String,
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
    /// Never negative.
    pub available_count: i64,
    pub credits: Vec<CodexResetCredit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct CodexResetRedeemRequest {
    #[ts(type = "string")]
    pub idempotency_key: uuid::Uuid,
    pub credit_id: Option<String>,
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
