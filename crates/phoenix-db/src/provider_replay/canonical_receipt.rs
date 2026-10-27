use chrono::{DateTime, Utc};
use phoenix_core::domain::{provider_replay::ProviderReplayUpdate, sm_state::ConvState};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

use crate::{DbError, DbResult};

#[derive(Debug, PartialEq)]
pub(super) struct CanonicalReceipt {
    state_hash: Vec<u8>,
    state_updated_at: String,
    replay_kind: &'static str,
    replay_hash: Option<Vec<u8>>,
}

fn command_hash(domain: &[u8], value: &impl Serialize) -> DbResult<Vec<u8>> {
    let mut value =
        serde_json::to_value(value).map_err(|error| DbError::Serialization(error.to_string()))?;
    value.sort_all_objects();
    let bytes =
        serde_json::to_vec(&value).map_err(|error| DbError::Serialization(error.to_string()))?;
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    Ok(hash.finalize().to_vec())
}

impl CanonicalReceipt {
    pub(super) fn new(
        state: &ConvState,
        timestamp: DateTime<Utc>,
        replay: Option<&ProviderReplayUpdate>,
    ) -> DbResult<Self> {
        let (replay_kind, replay_hash) = match replay {
            None => ("none", None),
            Some(ProviderReplayUpdate::Clear) => ("clear", None),
            Some(ProviderReplayUpdate::Anthropic(set)) => (
                "anthropic",
                Some(command_hash(b"canonical-tool-round/anthropic/v1\0", set)?),
            ),
            Some(ProviderReplayUpdate::Responses(set)) => (
                "responses",
                Some(command_hash(b"canonical-tool-round/responses/v1\0", set)?),
            ),
        };
        Ok(Self {
            state_hash: command_hash(b"canonical-tool-round/state/v1\0", state)?,
            state_updated_at: timestamp.to_rfc3339(),
            replay_kind,
            replay_hash,
        })
    }

    pub(super) async fn verify(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        conversation_id: &str,
        assistant_id: &str,
    ) -> DbResult<()> {
        let row = sqlx::query(
            "SELECT selected_state_sha256,selected_state_updated_at,replay_kind,replay_payload_sha256
             FROM canonical_tool_round_receipts WHERE conversation_id=? AND assistant_message_id=?",
        )
        .bind(conversation_id).bind(assistant_id).fetch_optional(&mut **tx).await?;
        let Some(row) = row else {
            return Err(DbError::Serialization(
                "retained round has no canonical checkpoint receipt".into(),
            ));
        };
        if row.try_get::<Vec<u8>, _>("selected_state_sha256")? != self.state_hash
            || row.try_get::<String, _>("selected_state_updated_at")? != self.state_updated_at
            || row.try_get::<String, _>("replay_kind")? != self.replay_kind
            || row.try_get::<Option<Vec<u8>>, _>("replay_payload_sha256")? != self.replay_hash
        {
            return Err(DbError::Serialization(
                "canonical checkpoint command differs".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn insert(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        conversation_id: &str,
        assistant_id: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO canonical_tool_round_receipts
             (conversation_id,assistant_message_id,selected_state_sha256,selected_state_updated_at,
              replay_kind,replay_payload_sha256) VALUES (?,?,?,?,?,?)",
        )
        .bind(conversation_id)
        .bind(assistant_id)
        .bind(&self.state_hash)
        .bind(&self.state_updated_at)
        .bind(self.replay_kind)
        .bind(&self.replay_hash)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_receipt_hashes_whole_payload_and_canonicalizes_nested_objects() {
        use phoenix_core::domain::provider_replay::ResponsesResponseSet;
        let state = ConvState::LlmRequesting { attempt: 1 };
        let timestamp = Utc::now();
        let mut left = ResponsesResponseSet {
            response_id: "response".into(),
            model: "model".into(),
            owner_message_id: "assistant".into(),
            public_content: vec![],
            output_items: vec![serde_json::from_str(
                r#"{"type":"reasoning","opaque":{"b":2,"a":[1,null]}}"#,
            )
            .unwrap()],
        };
        let mut right = left.clone();
        right.output_items =
            vec![
                serde_json::from_str(r#"{"opaque":{"a":[1,null],"b":2},"type":"reasoning"}"#)
                    .unwrap(),
            ];
        let first = CanonicalReceipt::new(
            &state,
            timestamp,
            Some(&ProviderReplayUpdate::Responses(left.clone())),
        )
        .unwrap();
        assert_eq!(
            first,
            CanonicalReceipt::new(
                &state,
                timestamp,
                Some(&ProviderReplayUpdate::Responses(right.clone()))
            )
            .unwrap()
        );
        right.output_items[0]["opaque"]["a"] = serde_json::json!([null, 1]);
        assert_ne!(
            first,
            CanonicalReceipt::new(
                &state,
                timestamp,
                Some(&ProviderReplayUpdate::Responses(right))
            )
            .unwrap()
        );
        left.owner_message_id = "other-assistant".into();
        assert_ne!(
            first,
            CanonicalReceipt::new(
                &state,
                timestamp,
                Some(&ProviderReplayUpdate::Responses(left))
            )
            .unwrap()
        );
    }

    #[test]
    fn command_hash_sorts_nested_object_keys_but_preserves_array_order() {
        let left: serde_json::Value =
            serde_json::from_str(r#"{"b":{"d":1,"c":2},"a":[1,2]}"#).unwrap();
        let right: serde_json::Value =
            serde_json::from_str(r#"{"a":[1,2],"b":{"c":2,"d":1}}"#).unwrap();
        assert_eq!(
            command_hash(b"test", &left).unwrap(),
            command_hash(b"test", &right).unwrap()
        );
        assert_ne!(
            command_hash(b"test", &left).unwrap(),
            command_hash(b"other", &right).unwrap()
        );
        assert_ne!(
            command_hash(b"test", &vec![1, 2]).unwrap(),
            command_hash(b"test", &vec![2, 1]).unwrap()
        );
    }
}
