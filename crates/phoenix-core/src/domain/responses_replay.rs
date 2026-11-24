//! Private output replay: Responses envelopes and signed Chat Completions tool calls.

use super::llm_types::ContentBlock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "api_format", content = "response", rename_all = "snake_case")]
pub enum ProviderResponseSet {
    Responses(ResponsesResponseSet),
    ChatCompletions(ChatResponseSet),
}

impl From<ResponsesResponseSet> for ProviderResponseSet {
    fn from(set: ResponsesResponseSet) -> Self {
        Self::Responses(set)
    }
}

impl From<ChatResponseSet> for ProviderResponseSet {
    fn from(set: ChatResponseSet) -> Self {
        Self::ChatCompletions(set)
    }
}

impl ProviderResponseSet {
    #[must_use]
    pub const fn as_responses(&self) -> Option<&ResponsesResponseSet> {
        match self {
            Self::Responses(set) => Some(set),
            Self::ChatCompletions(_) => None,
        }
    }

    #[must_use]
    pub const fn as_chat(&self) -> Option<&ChatResponseSet> {
        match self {
            Self::ChatCompletions(set) => Some(set),
            Self::Responses(_) => None,
        }
    }
}

impl ProviderResponseSet {
    #[must_use]
    pub fn response_id(&self) -> &str {
        match self {
            Self::Responses(set) => &set.response_id,
            Self::ChatCompletions(set) => &set.response_id,
        }
    }

    #[must_use]
    pub fn model(&self) -> &str {
        match self {
            Self::Responses(set) => &set.model,
            Self::ChatCompletions(set) => &set.model,
        }
    }

    #[must_use]
    pub fn owner_message_id(&self) -> &str {
        match self {
            Self::Responses(set) => &set.owner_message_id,
            Self::ChatCompletions(set) => &set.owner_message_id,
        }
    }

    #[must_use]
    pub fn public_content(&self) -> &[ContentBlock] {
        match self {
            Self::Responses(set) => &set.public_content,
            Self::ChatCompletions(set) => &set.public_content,
        }
    }

    /// # Errors
    /// Rejects incomplete identities or malformed private replay.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Responses(set) => set.validate(),
            Self::ChatCompletions(set) => set.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatToolSignature {
    pub tool_call_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatResponseSet {
    pub response_id: String,
    pub model: String,
    pub owner_message_id: String,
    pub public_content: Vec<ContentBlock>,
    pub tool_signatures: Vec<ChatToolSignature>,
}

impl ChatResponseSet {
    #[must_use]
    pub fn with_owner_message_id(mut self, owner_message_id: String) -> Self {
        self.owner_message_id = owner_message_id;
        self
    }

    /// # Errors
    /// Rejects incomplete identities, duplicate signatures, or signatures without an owning call.
    pub fn validate(&self) -> Result<(), String> {
        if self.response_id.is_empty() || self.model.is_empty() || self.owner_message_id.is_empty()
        {
            return Err("Chat Completions replay identity is incomplete".into());
        }
        if self.tool_signatures.is_empty() {
            return Err("Chat Completions replay has no signatures".into());
        }
        let mut seen = std::collections::HashSet::new();
        for signature in &self.tool_signatures {
            if signature.signature.is_empty()
                || !seen.insert(&signature.tool_call_id)
                || self.public_content.iter().filter(|block| {
                    matches!(block, ContentBlock::ToolUse { id, .. } if id == &signature.tool_call_id)
                }).count() != 1
            {
                return Err("Chat Completions signature is not bound to one authentic tool call".into());
            }
        }
        Ok(())
    }
}

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
