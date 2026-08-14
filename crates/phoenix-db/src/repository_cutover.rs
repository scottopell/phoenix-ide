use crate::git_repository_reconciliation::DormantGitRepositoryCanonicalReadinessEvidence;
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum RepositoryCutoverEvidenceError {
    #[error("repository cutover evidence artifact is invalid: {0}")]
    InvalidArtifact(String),
    #[error("repository cutover readiness is not eligible for authority binding")]
    ReadinessIneligible,
    #[error(
        "repository cutover candidate mismatch: readiness build is {readiness_sha}, census is {census_sha}"
    )]
    CandidateMismatch {
        readiness_sha: String,
        census_sha: String,
    },
}

type Result<T> = std::result::Result<T, RepositoryCutoverEvidenceError>;

#[derive(Debug, Deserialize)]
struct CensusEnvelope {
    authority_census: AuthorityCensusArtifact,
    #[serde(default, rename = "candidate_sha")]
    rejected_legacy_candidate_sha: Option<serde_json::Value>,
    #[serde(default, rename = "census_revision")]
    rejected_legacy_revision: Option<serde_json::Value>,
    #[serde(default, rename = "census_content_digest")]
    rejected_legacy_content_digest: Option<serde_json::Value>,
    #[serde(default, rename = "shadow_reference_count")]
    rejected_legacy_shadow_count: Option<serde_json::Value>,
    #[serde(default, rename = "project_authority_path_count")]
    rejected_legacy_project_count: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityCensusArtifact {
    conclusion: CensusConclusion,
    candidate_sha: String,
    revision: String,
    content_digest: String,
    shadow_reference_count: usize,
    project_authority_path_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CensusConclusion {
    Passed,
}

#[derive(Debug)]
pub(crate) struct VerifiedAuthorityCensus {
    candidate_sha: String,
    revision: String,
    content_digest: String,
    shadow_reference_count: usize,
    project_authority_path_count: usize,
}

#[allow(dead_code, reason = "future G1 consumes bound G0 evidence")]
#[derive(Debug)]
pub(crate) struct RepositoryCutoverG0Evidence {
    readiness: DormantGitRepositoryCanonicalReadinessEvidence,
    census: BoundAuthorityCensus,
}

#[allow(dead_code, reason = "future G1 consumes bound G0 evidence")]
#[derive(Debug)]
struct BoundAuthorityCensus {
    revision: String,
    content_digest: String,
    shadow_reference_count: usize,
    project_authority_path_count: usize,
}

#[allow(
    dead_code,
    reason = "future cutover deployment preflight supplies the census path"
)]
pub(crate) fn load_cutover_preflight_census(path: &Path) -> Result<VerifiedAuthorityCensus> {
    let bytes = std::fs::read(path).map_err(|error| invalid(format!("read failed: {error}")))?;
    parse_authority_census(&bytes)
}

fn parse_authority_census(bytes: &[u8]) -> Result<VerifiedAuthorityCensus> {
    let envelope: CensusEnvelope =
        serde_json::from_slice(bytes).map_err(|error| invalid(format!("JSON failed: {error}")))?;
    if [
        envelope.rejected_legacy_candidate_sha,
        envelope.rejected_legacy_revision,
        envelope.rejected_legacy_content_digest,
        envelope.rejected_legacy_shadow_count,
        envelope.rejected_legacy_project_count,
    ]
    .iter()
    .any(Option::is_some)
    {
        return Err(invalid(
            "legacy top-level census fields must not duplicate authority_census",
        ));
    }
    let census = envelope.authority_census;
    if !is_lower_hex(&census.candidate_sha, 40) {
        return Err(invalid(
            "candidate_sha must be exactly 40 lowercase hex bytes",
        ));
    }
    if census.revision.is_empty() {
        return Err(invalid("revision must be nonempty"));
    }
    if !is_lower_hex(&census.content_digest, 64) {
        return Err(invalid(
            "content_digest must be exactly 64 lowercase hex bytes",
        ));
    }
    if census.shadow_reference_count == 0 {
        return Err(invalid("shadow_reference_count must be nonzero"));
    }
    if census.project_authority_path_count == 0 {
        return Err(invalid("project_authority_path_count must be nonzero"));
    }
    let CensusConclusion::Passed = census.conclusion;
    Ok(VerifiedAuthorityCensus {
        candidate_sha: census.candidate_sha,
        revision: census.revision,
        content_digest: census.content_digest,
        shadow_reference_count: census.shadow_reference_count,
        project_authority_path_count: census.project_authority_path_count,
    })
}

#[allow(dead_code, reason = "future G1 binds G0 evidence")]
pub(crate) fn bind_repository_cutover_g0(
    readiness: DormantGitRepositoryCanonicalReadinessEvidence,
    census: VerifiedAuthorityCensus,
) -> Result<RepositoryCutoverG0Evidence> {
    let readiness_sha = readiness
        .eligible_clean_candidate_sha()
        .ok_or(RepositoryCutoverEvidenceError::ReadinessIneligible)?;
    if readiness_sha != census.candidate_sha {
        return Err(RepositoryCutoverEvidenceError::CandidateMismatch {
            readiness_sha: readiness_sha.to_string(),
            census_sha: census.candidate_sha,
        });
    }
    Ok(RepositoryCutoverG0Evidence {
        readiness,
        census: BoundAuthorityCensus {
            revision: census.revision,
            content_digest: census.content_digest,
            shadow_reference_count: census.shadow_reference_count,
            project_authority_path_count: census.project_authority_path_count,
        },
    })
}

fn invalid(message: impl Into<String>) -> RepositoryCutoverEvidenceError {
    RepositoryCutoverEvidenceError::InvalidArtifact(message.into())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn artifact(census: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "authority_census": census,
            "historical_sha": "ignored paired-restore evidence",
            "paired_offline_restore": "ignored",
        }))
        .unwrap()
    }

    fn passing_census() -> serde_json::Value {
        json!({
            "conclusion": "passed",
            "candidate_sha": SHA,
            "revision": "r1-authority-census-v1",
            "content_digest": DIGEST,
            "shadow_reference_count": 218,
            "project_authority_path_count": 909,
        })
    }

    #[test]
    fn census_parser_projects_only_independent_census_evidence() {
        let census = parse_authority_census(&artifact(&passing_census())).unwrap();
        assert_eq!(census.candidate_sha, SHA);
        assert_eq!(census.revision, "r1-authority-census-v1");
        assert_eq!(census.content_digest, DIGEST);
        assert_eq!(census.shadow_reference_count, 218);
        assert_eq!(census.project_authority_path_count, 909);
    }

    #[test]
    fn census_parser_rejects_absent_or_nonpassing_conclusion() {
        let mut absent = passing_census();
        absent.as_object_mut().unwrap().remove("conclusion");
        assert!(parse_authority_census(&artifact(&absent)).is_err());

        let mut failed = passing_census();
        failed["conclusion"] = json!("failed");
        assert!(parse_authority_census(&artifact(&failed)).is_err());
    }

    #[test]
    fn census_parser_rejects_invalid_identity_digest_revision_and_counts() {
        for (field, value) in [
            ("candidate_sha", json!("A".repeat(40))),
            ("revision", json!("")),
            ("content_digest", json!("0".repeat(63))),
            ("shadow_reference_count", json!(0)),
            ("project_authority_path_count", json!(0)),
        ] {
            let mut census = passing_census();
            census[field] = value;
            assert!(
                parse_authority_census(&artifact(&census)).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn binder_matches_exact_identity_and_removes_census_copy() {
        let readiness = crate::git_repository_reconciliation::test_readiness_evidence(
            crate::git_repository_reconciliation::TestReadinessBuild::ExactClean(SHA),
            crate::git_repository_reconciliation::DormantGitRepositoryR1Eligibility::Eligible,
        );
        let census = parse_authority_census(&artifact(&passing_census())).unwrap();
        let bound = bind_repository_cutover_g0(readiness, census).unwrap();

        assert_eq!(bound.readiness.eligible_clean_candidate_sha(), Some(SHA));
        assert_eq!(bound.census.revision, "r1-authority-census-v1");
        assert_eq!(bound.census.content_digest, DIGEST);
        assert_eq!(bound.census.shadow_reference_count, 218);
        assert_eq!(bound.census.project_authority_path_count, 909);
    }

    #[test]
    fn binder_rejects_candidate_mismatch() {
        let readiness = crate::git_repository_reconciliation::test_readiness_evidence(
            crate::git_repository_reconciliation::TestReadinessBuild::ExactClean(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            crate::git_repository_reconciliation::DormantGitRepositoryR1Eligibility::Eligible,
        );
        let census = parse_authority_census(&artifact(&passing_census())).unwrap();

        assert!(matches!(
            bind_repository_cutover_g0(readiness, census),
            Err(RepositoryCutoverEvidenceError::CandidateMismatch { .. })
        ));
    }

    #[test]
    fn binder_rejects_dirty_unavailable_and_ineligible_readiness() {
        use crate::git_repository_reconciliation::{
            DormantGitRepositoryR1Eligibility as Eligibility, TestReadinessBuild as Build,
        };
        for readiness in [
            crate::git_repository_reconciliation::test_readiness_evidence(
                Build::Dirty(SHA),
                Eligibility::Eligible,
            ),
            crate::git_repository_reconciliation::test_readiness_evidence(
                Build::Unavailable,
                Eligibility::Eligible,
            ),
            crate::git_repository_reconciliation::test_readiness_evidence(
                Build::ExactClean(SHA),
                Eligibility::Ineligible,
            ),
        ] {
            let census = parse_authority_census(&artifact(&passing_census())).unwrap();
            assert!(matches!(
                bind_repository_cutover_g0(readiness, census),
                Err(RepositoryCutoverEvidenceError::ReadinessIneligible)
            ));
        }
    }

    #[test]
    fn census_projection_rejects_legacy_top_level_census_duplicates() {
        for field in [
            "candidate_sha",
            "census_revision",
            "census_content_digest",
            "shadow_reference_count",
            "project_authority_path_count",
        ] {
            let mut envelope: serde_json::Value =
                serde_json::from_slice(&artifact(&passing_census())).unwrap();
            envelope[field] = json!("duplicate");
            assert!(parse_authority_census(&serde_json::to_vec(&envelope).unwrap()).is_err());
        }
    }

    #[test]
    fn census_projection_rejects_unknown_fields() {
        let mut census = passing_census();
        census["paired_offline_restore"] = json!("passed");
        assert!(parse_authority_census(&artifact(&census)).is_err());
    }
}
