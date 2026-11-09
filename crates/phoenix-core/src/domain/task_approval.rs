//! Typed identity vocabulary for native task approval capture.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyIdentity> {
        nonempty(value.into()).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyIdentity> {
        nonempty(value.into()).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolUseId(String);

impl ToolUseId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyIdentity> {
        nonempty(value.into()).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCallIdentity {
    pub conversation_id: ConversationId,
    pub assistant_message_id: MessageId,
    pub tool_use_id: ToolUseId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberId(String);

impl MemberId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyIdentity> {
        nonempty(value.into()).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CanonicalProposalId(String);

impl CanonicalProposalId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyIdentity> {
        nonempty(value.into()).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalOwner {
    pub conversation_id: ConversationId,
    pub member_id: MemberId,
    pub proposal_id: CanonicalProposalId,
}

/// Native approval is deliberately scoped to continuation in the owning target.
/// Fresh-start is not a representable native target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalScope {
    ContinueHere,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeApprovalCapture {
    pub source: SourceCallIdentity,
    pub target: ProposalOwner,
    pub scope: ApprovalScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("identity must not be empty")]
pub struct EmptyIdentity;

fn nonempty(value: String) -> Result<String, EmptyIdentity> {
    if value.is_empty() {
        Err(EmptyIdentity)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(conversation: &str, message: &str, tool_use: &str) -> SourceCallIdentity {
        SourceCallIdentity {
            conversation_id: ConversationId::new(conversation).unwrap(),
            assistant_message_id: MessageId::new(message).unwrap(),
            tool_use_id: ToolUseId::new(tool_use).unwrap(),
        }
    }

    fn target(conversation: &str, member: &str, proposal: &str) -> ProposalOwner {
        ProposalOwner {
            conversation_id: ConversationId::new(conversation).unwrap(),
            member_id: MemberId::new(member).unwrap(),
            proposal_id: CanonicalProposalId::new(proposal).unwrap(),
        }
    }

    #[test]
    fn capture_identity_includes_full_source_and_target_ownership() {
        let captured = NativeApprovalCapture {
            source: source("source-conversation", "assistant-message", "tool-use"),
            target: target("owner-conversation", "member", "proposal"),
            scope: ApprovalScope::ContinueHere,
        };
        assert_eq!(captured.source.assistant_message_id.as_str(), "assistant-message");
        assert_eq!(captured.target.member_id.as_str(), "member");

        let other_owner = NativeApprovalCapture {
            target: target("different-owner", "member", "proposal"),
            ..captured.clone()
        };
        let other_member = NativeApprovalCapture {
            target: target("owner-conversation", "different-member", "proposal"),
            ..captured.clone()
        };
        let other_proposal = NativeApprovalCapture {
            target: target("owner-conversation", "member", "different-proposal"),
            ..captured.clone()
        };
        assert_ne!(captured, other_owner);
        assert_ne!(captured, other_member);
        assert_ne!(captured, other_proposal);
        assert_ne!(captured.source, source("source-conversation", "other-message", "tool-use"));
        assert_ne!(captured.source, source("source-conversation", "assistant-message", "other-tool"));
    }

    #[test]
    fn identity_constructors_reject_empty_values() {
        assert!(ConversationId::new("").is_err());
        assert!(MessageId::new("").is_err());
        assert!(ToolUseId::new("").is_err());
        assert!(MemberId::new("").is_err());
        assert!(CanonicalProposalId::new("").is_err());
    }

    #[test]
    fn native_scope_has_no_fresh_start_variant() {
        let scope = ApprovalScope::ContinueHere;
        assert_eq!(scope, ApprovalScope::ContinueHere);
    }
}
