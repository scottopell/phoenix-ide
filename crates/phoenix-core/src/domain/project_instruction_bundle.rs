//! Immutable snapshots of project guidance and the available skill catalog.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

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

/// How one instruction source differs from the comparison bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ProjectInstructionSourceChangeKind {
    Added,
    Changed,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProjectGuidanceSourceChange {
    pub relative_path: String,
    pub status: ProjectInstructionSourceChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProjectSkillSourceChange {
    pub name: String,
    pub status: ProjectInstructionSourceChangeKind,
}

/// Content-free comparison of two project-instruction bundles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProjectInstructionChangeManifest {
    pub guidance: Vec<ProjectGuidanceSourceChange>,
    pub skills: Vec<ProjectSkillSourceChange>,
    pub unchanged_guidance_count: u64,
    pub unchanged_skill_count: u64,
}

impl ProjectInstructionChangeManifest {
    #[must_use]
    pub fn between(
        comparison: &ProjectInstructionBundle,
        candidate: &ProjectInstructionBundle,
    ) -> Self {
        let old_guidance: BTreeMap<_, _> = comparison
            .guidance
            .iter()
            .map(|entry| (entry.relative_path.as_str(), entry.content_hash.as_str()))
            .collect();
        let new_guidance: BTreeMap<_, _> = candidate
            .guidance
            .iter()
            .map(|entry| (entry.relative_path.as_str(), entry.content_hash.as_str()))
            .collect();
        let old_skills: BTreeMap<_, _> = comparison
            .skills
            .iter()
            .map(|entry| (entry.name.as_str(), entry.content_hash.as_str()))
            .collect();
        let new_skills: BTreeMap<_, _> = candidate
            .skills
            .iter()
            .map(|entry| (entry.name.as_str(), entry.content_hash.as_str()))
            .collect();

        let (guidance, unchanged_guidance_count) =
            diff_sources(&old_guidance, &new_guidance).into_iter().fold(
                (Vec::new(), 0_u64),
                |(mut changed, unchanged), (name, status)| {
                    match status {
                        Some(status) => changed.push(ProjectGuidanceSourceChange {
                            relative_path: name,
                            status,
                        }),
                        None => return (changed, unchanged + 1),
                    }
                    (changed, unchanged)
                },
            );
        let (skills, unchanged_skill_count) =
            diff_sources(&old_skills, &new_skills).into_iter().fold(
                (Vec::new(), 0_u64),
                |(mut changed, unchanged), (name, status)| {
                    match status {
                        Some(status) => changed.push(ProjectSkillSourceChange { name, status }),
                        None => return (changed, unchanged + 1),
                    }
                    (changed, unchanged)
                },
            );

        Self {
            guidance,
            skills,
            unchanged_guidance_count,
            unchanged_skill_count,
        }
    }
}

fn diff_sources(
    old: &BTreeMap<&str, &str>,
    new: &BTreeMap<&str, &str>,
) -> Vec<(String, Option<ProjectInstructionSourceChangeKind>)> {
    let mut names: Vec<_> = old.keys().chain(new.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let status = match (old.get(name), new.get(name)) {
                (None, Some(_)) => Some(ProjectInstructionSourceChangeKind::Added),
                (Some(_), None) => Some(ProjectInstructionSourceChangeKind::Removed),
                (Some(old_hash), Some(new_hash)) if old_hash != new_hash => {
                    Some(ProjectInstructionSourceChangeKind::Changed)
                }
                (Some(_), Some(_)) => None,
                (None, None) => unreachable!("name came from one of the maps"),
            };
            (name.to_string(), status)
        })
        .collect()
}

/// Conversation-scoped refresh projection. It deliberately contains source
/// identities and hashes-derived statuses, never instruction contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ProjectInstructionRefreshStatus {
    pub active_bundle_id: String,
    pub queued_bundle_id: Option<String>,
    pub candidate_bundle_id: Option<String>,
    pub changed_manifest: ProjectInstructionChangeManifest,
    pub estimated_rewarm_tokens: u64,
    pub rewarm_tokens_are_estimate: bool,
    pub rewarm_estimate_notice: String,
    pub is_queued: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(guidance: &[(&str, &str)], skills: &[(&str, &str)]) -> ProjectInstructionBundle {
        ProjectInstructionBundle {
            id: "bundle".into(),
            conversation_id: "conversation".into(),
            role: ProjectInstructionBundleRole::Active,
            estimated_tokens: 1,
            created_at: Utc::now(),
            guidance: guidance
                .iter()
                .map(|(path, hash)| ProjectGuidanceSnapshot {
                    relative_path: (*path).into(),
                    content: format!("secret-{path}"),
                    content_hash: (*hash).into(),
                })
                .collect(),
            skills: skills
                .iter()
                .map(|(name, hash)| ProjectSkillSnapshot {
                    name: (*name).into(),
                    description: format!("secret-{name}"),
                    source_label: "test".into(),
                    content_hash: (*hash).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn manifest_reports_guidance_and_skill_source_statuses() {
        let old = bundle(
            &[("AGENTS.md", "a"), ("gone.md", "g")],
            &[("same", "s"), ("gone", "g")],
        );
        let new = bundle(
            &[("AGENTS.md", "b"), ("new.md", "n")],
            &[("same", "s"), ("new", "n")],
        );

        let manifest = ProjectInstructionChangeManifest::between(&old, &new);

        assert_eq!(manifest.guidance.len(), 3);
        assert_eq!(
            manifest.guidance[0].status,
            ProjectInstructionSourceChangeKind::Changed
        );
        assert_eq!(
            manifest.guidance[1].status,
            ProjectInstructionSourceChangeKind::Removed
        );
        assert_eq!(
            manifest.guidance[2].status,
            ProjectInstructionSourceChangeKind::Added
        );
        assert_eq!(manifest.skills.len(), 2);
        assert_eq!(manifest.unchanged_skill_count, 1);
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains("secret-"));
    }
}
