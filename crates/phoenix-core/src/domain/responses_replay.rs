//! Private Responses output envelopes, retained as indivisible provider items.

use super::llm_types::ContentBlock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponsesResponseSet {
    pub response_id: String,
    pub model: String,
    pub owner_message_id: String,
    pub public_content: Vec<ContentBlock>,
    pub output_items: Vec<serde_json::Value>,
}

impl ResponsesResponseSet {
    #[must_use]
    pub fn with_owner_message_id(mut self, owner_message_id: String) -> Self {
        self.owner_message_id = owner_message_id;
        self
    }

    /// # Errors
    /// Rejects incomplete identities or malformed opaque output envelopes.
    pub fn validate(&self) -> Result<(), String> {
        if self.response_id.is_empty() || self.model.is_empty() || self.owner_message_id.is_empty()
        {
            return Err("Responses replay identity is incomplete".into());
        }
        if self.output_items.is_empty()
            || self.output_items.iter().any(|item| {
                !item.is_object()
                    || item
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .is_none()
            })
        {
            return Err("Responses replay contains malformed output envelopes".into());
        }
        Ok(())
    }
}
