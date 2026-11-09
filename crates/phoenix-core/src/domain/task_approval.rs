//! Canonical identity for a blocking task proposal.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::task_source::TaskSource;

/// An occurrence is the assistant message AND its tool-use, not a task path or
/// a generated proposal handle. Re-proposing identical Markdown has new identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalOccurrence {
    pub assistant_message_id: String,
    pub tool_use_id: String,
}

impl ProposalOccurrence {
    /// Only the canonical transition should construct this; persisted legacy
    /// pending states have `None`, never reconstructed from transcript text.
    #[must_use]
    pub fn capture(assistant_message_id: &str, tool_use_id: &str) -> Option<Self> {
        if assistant_message_id.trim().is_empty() || tool_use_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            assistant_message_id: assistant_message_id.to_owned(),
            tool_use_id: tool_use_id.to_owned(),
        })
    }
}

/// Single whole-read authority carried by a pending approval. The existing
/// state fields hold the exact reviewed body and path; no second body is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedProposal {
    pub occurrence: ProposalOccurrence,
    pub kind: ReviewedTaskKind,
    pub body_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewedTaskKind {
    Taskmd,
    PlainMarkdown,
}

impl ReviewedProposal {
    #[must_use]
    pub fn capture(occurrence: ProposalOccurrence, source: &TaskSource, body: &str) -> Self {
        let kind = match source {
            TaskSource::Taskmd { .. } => ReviewedTaskKind::Taskmd,
            TaskSource::PlainMarkdown { .. } => ReviewedTaskKind::PlainMarkdown,
        };
        let body_sha256 = format!("{:x}", Sha256::digest(body.as_bytes()));
        Self {
            occurrence,
            kind,
            body_sha256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrence_changes_on_same_body_reproposal() {
        let first = ProposalOccurrence::capture("assistant-1", "tool-1").unwrap();
        let second = ProposalOccurrence::capture("assistant-2", "tool-1").unwrap();
        let source = TaskSource::detect("plan.md").unwrap();
        let a = ReviewedProposal::capture(first, &source, "# Identical\n");
        let b = ReviewedProposal::capture(second, &source, "# Identical\n");
        assert_eq!(a.body_sha256, b.body_sha256);
        assert_ne!(a.occurrence, b.occurrence);
        assert!(ProposalOccurrence::capture("", "tool").is_none());
        assert!(ProposalOccurrence::capture("assistant", " ").is_none());
    }

    #[test]
    fn reviewed_bytes_and_kind_are_distinguished() {
        let occurrence = ProposalOccurrence::capture("assistant", "tool").unwrap();
        let plain = TaskSource::detect("plan.md").unwrap();
        let taskmd = TaskSource::detect("53019-p1-ready--plan.md").unwrap();
        let a = ReviewedProposal::capture(occurrence.clone(), &plain, "# Plan\n");
        let b = ReviewedProposal::capture(occurrence.clone(), &plain, "# Plan");
        let c = ReviewedProposal::capture(occurrence, &taskmd, "# Plan\n");
        assert_ne!(a.body_sha256, b.body_sha256);
        assert_eq!(a.body_sha256, c.body_sha256);
        assert_ne!(a.kind, c.kind);
    }
}
