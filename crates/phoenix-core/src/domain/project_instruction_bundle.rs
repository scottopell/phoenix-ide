//! Immutable snapshots of project guidance and the available skill catalog.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectInstructionBundleRole {
    Active,
    Queued,
    Candidate,
    Historical,
}

impl ProjectInstructionBundleRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Queued => "queued",
            Self::Candidate => "candidate",
            Self::Historical => "historical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGuidanceSnapshot {
    pub relative_path: String,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSkillSnapshot {
    pub name: String,
    pub description: String,
    pub source_label: String,
    pub content_hash: String,
}

/// A complete, ordered project-instruction value. Callers replace bundles rather
/// than mutating their child collections, so a confirmed snapshot stays exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInstructionBundle {
    pub id: String,
    pub conversation_id: String,
    pub role: ProjectInstructionBundleRole,
    pub estimated_tokens: u64,
    pub created_at: DateTime<Utc>,
    pub guidance: Vec<ProjectGuidanceSnapshot>,
    pub skills: Vec<ProjectSkillSnapshot>,
}

/// Content supplied when creating a new immutable bundle version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProjectInstructionBundle {
    pub estimated_tokens: u64,
    pub guidance: Vec<ProjectGuidanceSnapshot>,
    pub skills: Vec<ProjectSkillSnapshot>,
}
