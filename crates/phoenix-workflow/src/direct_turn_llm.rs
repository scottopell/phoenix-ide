use crate::ObservationId;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use thiserror::Error;

pub const INTENT_CODEC_FAMILY: &str = "direct_turn.llm_intent";
pub const OBSERVATION_CODEC_FAMILY: &str = "direct_turn.llm_observation";
pub const RECEIPT_CODEC_FAMILY: &str = "direct_turn.llm_receipt";
pub const RECEIPT_EVENT_CODEC_FAMILY: &str = "direct_turn.llm_receipt_event";
pub const CODEC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LlmPayloadError {
    #[error("payload fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
    #[error("provider result belongs to request {actual}, not {expected}")]
    RequestMismatch { expected: String, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparedLlmRequest {
    fingerprint: String,
    canonical_payload: Vec<u8>,
}

impl PreparedLlmRequest {
    #[must_use]
    pub fn new(canonical_payload: Vec<u8>) -> Self {
        Self {
            fingerprint: fingerprint(&canonical_payload),
            canonical_payload,
        }
    }

    /// Rebuilds a prepared request from its persisted representation.
    ///
    /// # Errors
    ///
    /// Returns [`LlmPayloadError::FingerprintMismatch`] when the persisted
    /// fingerprint does not match the canonical payload.
    pub fn rehydrate(
        fingerprint: String,
        canonical_payload: Vec<u8>,
    ) -> Result<Self, LlmPayloadError> {
        let expected = self::fingerprint(&canonical_payload);
        if fingerprint != expected {
            return Err(LlmPayloadError::FingerprintMismatch {
                expected,
                actual: fingerprint,
            });
        }
        Ok(Self {
            fingerprint,
            canonical_payload,
        })
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn canonical_payload(&self) -> &[u8] {
        &self.canonical_payload
    }
}

#[derive(Deserialize)]
struct PersistedPreparedLlmRequest {
    fingerprint: String,
    canonical_payload: Vec<u8>,
}

impl<'de> Deserialize<'de> for PreparedLlmRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedPreparedLlmRequest::deserialize(deserializer)?;
        Self::rehydrate(persisted.fingerprint, persisted.canonical_payload)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmEffectIntent {
    pub turn_id: u64,
    pub request: PreparedLlmRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderFailureKind {
    RateLimited,
    UsageLimitReached,
    Server,
    InvalidResponse,
    Overloaded,
    Network,
    TokenBudgetExceeded,
    Authentication,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmResultKind {
    Success,
    Failure(ProviderFailureKind),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmResultObservation {
    request_fingerprint: String,
    result_fingerprint: String,
    kind: LlmResultKind,
    canonical_payload: Vec<u8>,
}

impl LlmResultObservation {
    #[must_use]
    pub fn success(request: &PreparedLlmRequest, canonical_payload: Vec<u8>) -> Self {
        Self::new(request, LlmResultKind::Success, canonical_payload)
    }

    #[must_use]
    pub fn failure(
        request: &PreparedLlmRequest,
        kind: ProviderFailureKind,
        canonical_payload: Vec<u8>,
    ) -> Self {
        Self::new(request, LlmResultKind::Failure(kind), canonical_payload)
    }

    fn new(request: &PreparedLlmRequest, kind: LlmResultKind, canonical_payload: Vec<u8>) -> Self {
        Self {
            request_fingerprint: request.fingerprint().to_string(),
            result_fingerprint: fingerprint(&canonical_payload),
            kind,
            canonical_payload,
        }
    }

    /// Rebuilds an observation from its persisted representation.
    ///
    /// # Errors
    ///
    /// Returns [`LlmPayloadError::FingerprintMismatch`] when the persisted
    /// result fingerprint does not match the canonical result payload.
    pub fn rehydrate(
        request_fingerprint: String,
        result_fingerprint: String,
        kind: LlmResultKind,
        canonical_payload: Vec<u8>,
    ) -> Result<Self, LlmPayloadError> {
        let expected = fingerprint(&canonical_payload);
        if result_fingerprint != expected {
            return Err(LlmPayloadError::FingerprintMismatch {
                expected,
                actual: result_fingerprint,
            });
        }
        Ok(Self {
            request_fingerprint,
            result_fingerprint,
            kind,
            canonical_payload,
        })
    }

    /// Checks that this result was observed for the supplied prepared request.
    ///
    /// # Errors
    ///
    /// Returns [`LlmPayloadError::RequestMismatch`] when the observation names
    /// a different prepared request fingerprint.
    pub fn validate_request(&self, request: &PreparedLlmRequest) -> Result<(), LlmPayloadError> {
        if self.request_fingerprint != request.fingerprint() {
            return Err(LlmPayloadError::RequestMismatch {
                expected: request.fingerprint().to_string(),
                actual: self.request_fingerprint.clone(),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    #[must_use]
    pub fn result_fingerprint(&self) -> &str {
        &self.result_fingerprint
    }

    #[must_use]
    pub fn kind(&self) -> &LlmResultKind {
        &self.kind
    }

    #[must_use]
    pub fn canonical_payload(&self) -> &[u8] {
        &self.canonical_payload
    }
}

#[derive(Deserialize)]
struct PersistedLlmResultObservation {
    request_fingerprint: String,
    result_fingerprint: String,
    kind: LlmResultKind,
    canonical_payload: Vec<u8>,
}

impl<'de> Deserialize<'de> for LlmResultObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedLlmResultObservation::deserialize(deserializer)?;
        Self::rehydrate(
            persisted.request_fingerprint,
            persisted.result_fingerprint,
            persisted.kind,
            persisted.canonical_payload,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResultReceipt {
    pub observation_id: ObservationId,
    pub result_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmResultReducerEvent {
    ResultObserved(LlmResultReceipt),
}

fn fingerprint(payload: &[u8]) -> String {
    Sha256::digest(payload)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_request_rejects_corrupt_persisted_fingerprint() {
        let json = br#"{"fingerprint":"wrong","canonical_payload":[1,2,3]}"#;
        let error = serde_json::from_slice::<PreparedLlmRequest>(json).unwrap_err();
        assert!(error.to_string().contains("fingerprint mismatch"));
    }

    #[test]
    fn observation_is_bound_to_one_request_and_validates_on_decode() {
        let request = PreparedLlmRequest::new(br#"{"prompt":"one"}"#.to_vec());
        let other = PreparedLlmRequest::new(br#"{"prompt":"two"}"#.to_vec());
        let observation = LlmResultObservation::success(&request, br#"{"answer":"ok"}"#.to_vec());

        assert_eq!(observation.validate_request(&request), Ok(()));
        assert!(matches!(
            observation.validate_request(&other),
            Err(LlmPayloadError::RequestMismatch { .. })
        ));

        let encoded = serde_json::to_vec(&observation).unwrap();
        assert_eq!(
            serde_json::from_slice::<LlmResultObservation>(&encoded).unwrap(),
            observation
        );
    }

    #[test]
    fn receipt_references_observation_without_copying_result_payload() {
        let receipt = LlmResultReceipt {
            observation_id: ObservationId(9),
            result_fingerprint: "result-fingerprint".to_string(),
        };
        let encoded = serde_json::to_value(&receipt).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 2);
        assert!(encoded.get("canonical_payload").is_none());
    }
}
