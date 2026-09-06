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

#[must_use]
pub fn intent_codec() -> crate::CodecRef {
    crate::CodecRef {
        family: INTENT_CODEC_FAMILY,
        version: CODEC_VERSION,
    }
}

#[must_use]
pub fn observation_codec() -> crate::CodecRef {
    crate::CodecRef {
        family: OBSERVATION_CODEC_FAMILY,
        version: CODEC_VERSION,
    }
}

#[must_use]
pub fn receipt_codec() -> crate::CodecRef {
    crate::CodecRef {
        family: RECEIPT_CODEC_FAMILY,
        version: CODEC_VERSION,
    }
}

#[must_use]
pub fn receipt_event_codec() -> crate::CodecRef {
    crate::CodecRef {
        family: RECEIPT_EVENT_CODEC_FAMILY,
        version: CODEC_VERSION,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LlmPayloadError {
    #[error("payload fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
    #[error("provider result belongs to request {actual}, not {expected}")]
    RequestMismatch { expected: String, actual: String },
}

#[derive(Debug, Error)]
pub enum LlmObservationDecodeError {
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Payload(#[from] LlmPayloadError),
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct LlmEffectIntent {
    #[serde(
        serialize_with = "serialize_turn_id",
        deserialize_with = "deserialize_turn_id"
    )]
    pub turn_id: crate::direct_turn::TurnAuthorityId,
    pub request: PreparedLlmRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmResultObservation {
    request_fingerprint: String,
    result_fingerprint: String,
    canonical_payload: Vec<u8>,
}

impl LlmResultObservation {
    #[must_use]
    pub fn new(request: &PreparedLlmRequest, canonical_payload: Vec<u8>) -> Self {
        Self {
            request_fingerprint: request.fingerprint().to_string(),
            result_fingerprint: fingerprint(&canonical_payload),
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
        request: &PreparedLlmRequest,
        request_fingerprint: String,
        result_fingerprint: String,
        canonical_payload: Vec<u8>,
    ) -> Result<Self, LlmPayloadError> {
        if request_fingerprint != request.fingerprint() {
            return Err(LlmPayloadError::RequestMismatch {
                expected: request.fingerprint().to_string(),
                actual: request_fingerprint,
            });
        }
        let expected = fingerprint(&canonical_payload);
        if result_fingerprint != expected {
            return Err(LlmPayloadError::FingerprintMismatch {
                expected,
                actual: result_fingerprint,
            });
        }
        Ok(Self {
            request_fingerprint: request.fingerprint().to_string(),
            result_fingerprint,
            canonical_payload,
        })
    }

    /// Decodes and binds persisted observation bytes to an expected request.
    ///
    /// # Errors
    ///
    /// Returns a serialization error for an incompatible payload and a typed
    /// payload error when either fingerprint does not match.
    pub fn decode_exact(
        request: &PreparedLlmRequest,
        encoded: &[u8],
    ) -> Result<Self, LlmObservationDecodeError> {
        let persisted: PersistedLlmResultObservation = serde_json::from_slice(encoded)?;
        Self::rehydrate(
            request,
            persisted.request_fingerprint,
            persisted.result_fingerprint,
            persisted.canonical_payload,
        )
        .map_err(Into::into)
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
    pub fn canonical_payload(&self) -> &[u8] {
        &self.canonical_payload
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedLlmResultObservation {
    request_fingerprint: String,
    result_fingerprint: String,
    canonical_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmResultReceipt {
    observation_id: ObservationId,
}

impl LlmResultReceipt {
    #[must_use]
    pub fn from_observation(observation_id: ObservationId) -> Self {
        Self { observation_id }
    }

    #[must_use]
    pub fn observation_id(&self) -> ObservationId {
        self.observation_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmResultReducerEvent {
    ResultObserved(LlmResultReceipt),
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_turn_id<S>(
    id: &crate::direct_turn::TurnAuthorityId,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u64(id.0)
}

fn deserialize_turn_id<'de, D>(
    deserializer: D,
) -> Result<crate::direct_turn::TurnAuthorityId, D::Error>
where
    D: Deserializer<'de>,
{
    u64::deserialize(deserializer).map(crate::direct_turn::TurnAuthorityId)
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
    fn intent_decoder_rejects_unknown_fields() {
        let request = PreparedLlmRequest::new(vec![1]);
        let intent = LlmEffectIntent {
            turn_id: crate::direct_turn::TurnAuthorityId(7),
            request,
        };
        let mut value = serde_json::to_value(intent).unwrap();
        value["future"] = serde_json::json!(1);

        assert!(serde_json::from_value::<LlmEffectIntent>(value)
            .unwrap_err()
            .to_string()
            .contains("unknown field"));
    }

    #[test]
    fn observation_is_bound_to_one_request_and_validates_on_decode() {
        let request = PreparedLlmRequest::new(br#"{"prompt":"one"}"#.to_vec());
        let other = PreparedLlmRequest::new(br#"{"prompt":"two"}"#.to_vec());
        let observation = LlmResultObservation::new(&request, br#"{"answer":"ok"}"#.to_vec());
        let encoded = serde_json::to_vec(&observation).unwrap();

        assert_eq!(
            LlmResultObservation::decode_exact(&request, &encoded).unwrap(),
            observation
        );
        assert!(matches!(
            LlmResultObservation::decode_exact(&other, &encoded),
            Err(LlmObservationDecodeError::Payload(
                LlmPayloadError::RequestMismatch { .. }
            ))
        ));
    }

    #[test]
    fn durable_decoders_reject_unknown_fields() {
        let request_json = br#"{"fingerprint":"wrong","canonical_payload":[],"future":1}"#;
        assert!(serde_json::from_slice::<PreparedLlmRequest>(request_json)
            .unwrap_err()
            .to_string()
            .contains("unknown field"));

        let request = PreparedLlmRequest::new(vec![1]);
        let observation = LlmResultObservation::new(&request, vec![2]);
        let mut value = serde_json::to_value(observation).unwrap();
        value["future"] = serde_json::json!(1);
        assert!(
            LlmResultObservation::decode_exact(&request, &serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn receipt_references_observation_without_copying_result_payload() {
        let receipt = LlmResultReceipt::from_observation(ObservationId(9));
        let encoded = serde_json::to_value(&receipt).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 1);
        assert_eq!(receipt.observation_id(), ObservationId(9));
        assert!(encoded.get("canonical_payload").is_none());
    }
}
