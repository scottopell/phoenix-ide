use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt as _;

use super::RuntimeManager;
use crate::db::{ConvMode, Database};
use crate::work_scope::WorkScope;

pub(crate) const POLL_BASE_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const POLL_JITTER: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PrBranchCandidate {
    repository_identity: String,
    branch_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PollTarget {
    work_scope: WorkScope,
    worktree_path: PathBuf,
    repository_identity: Option<String>,
    candidate_branches: Vec<PrBranchCandidate>,
}

pub(crate) fn github_repo_identifier(path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_end_matches(".git")
        .to_string();
    let slug = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("http://github.com/"))
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let mut parts = slug.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    (parts.next().is_none() && !owner.is_empty() && !repo.is_empty())
        .then(|| format!("{owner}/{repo}"))
}

pub(crate) fn repository_identity_is_structurally_local_path(identity: &str) -> bool {
    Path::new(identity).is_absolute()
}

fn candidate_for_conv_branch(conv: &crate::db::Conversation) -> Option<PrBranchCandidate> {
    let branch_name = conv.conv_mode.branch_name()?;
    let worktree_path = conv.conv_mode.worktree_path()?;
    let repo_root = phoenix_core::git::detect_git_repo_root(Path::new(worktree_path))?;
    let repository_identity = github_repo_identifier(Path::new(&repo_root))?;
    Some(PrBranchCandidate {
        repository_identity,
        branch_name: branch_name.to_string(),
    })
}

fn normalize_candidate_for_target_repo(
    repository_identity: Option<&str>,
    candidate: PrBranchCandidate,
) -> Option<PrBranchCandidate> {
    match repository_identity {
        Some(repo) if candidate.repository_identity == repo => Some(candidate),
        Some(repo)
            if repository_identity_is_structurally_local_path(&candidate.repository_identity) =>
        {
            Some(PrBranchCandidate {
                repository_identity: repo.to_string(),
                branch_name: candidate.branch_name,
            })
        }
        Some(_) => None,
        None => Some(candidate),
    }
}

#[cfg(test)]
fn inference_input_from_candidates(
    repository_identity: Option<&str>,
    candidates: &[PrBranchCandidate],
) -> phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
    phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
        latest_observed_branch: candidates.iter().find_map(|candidate| {
            repository_identity
                .is_none_or(|repo| repo == candidate.repository_identity)
                .then(
                    || phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                        repository_identity: candidate.repository_identity.clone(),
                        branch_name: candidate.branch_name.clone(),
                    },
                )
        }),
    }
}

fn observations_for_target_repo(
    repository_identity: Option<&str>,
    observations: Vec<crate::db::WorkScopePrObservation>,
) -> Vec<crate::db::WorkScopePrObservation> {
    match repository_identity {
        Some(repo) => observations
            .into_iter()
            .filter(|observation| {
                format!("{}/{}", observation.repo_owner, observation.repo_name) == repo
            })
            .collect(),
        None => observations,
    }
}

pub(crate) fn interval_with_jitter(
    base: Duration,
    jitter: Duration,
    jitter_offset_secs: u64,
) -> Duration {
    let span = jitter.as_secs().saturating_mul(2);
    let clamped = jitter_offset_secs.min(span);
    base.saturating_sub(jitter) + Duration::from_secs(clamped)
}

fn next_poll_delay() -> Duration {
    let jitter_offset = rand::rng().random_range(0..=POLL_JITTER.as_secs().saturating_mul(2));
    interval_with_jitter(POLL_BASE_INTERVAL, POLL_JITTER, jitter_offset)
}

pub(crate) async fn run(manager: Arc<RuntimeManager>) {
    loop {
        tokio::time::sleep(next_poll_delay()).await;
        if let Err(err) = poll_once(&manager).await {
            tracing::debug!(error = %err, "background PR status poll failed");
        }
    }
}

async fn poll_once(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let targets = collect_targets(manager.db()).await?;
    for target in targets {
        poll_target(manager, target).await;
    }
    Ok(())
}

async fn collect_targets(db: &Database) -> Result<Vec<PollTarget>, String> {
    let conversations = db
        .list_conversations()
        .await
        .map_err(|err| err.to_string())?;
    let mut seen = HashSet::new();
    let mut targets = Vec::new();

    for conv in conversations {
        let worktree_path = match &conv.conv_mode {
            ConvMode::Work { worktree_path, .. } | ConvMode::Branch { worktree_path, .. } => {
                worktree_path.to_string()
            }
            _ => continue,
        };
        let work_scope = WorkScope::resolve(&conv.id, Some(Path::new(&worktree_path)));
        if !seen.insert(work_scope.clone()) {
            continue;
        }
        let repository_identity =
            phoenix_core::git::detect_git_repo_root(Path::new(&worktree_path))
                .and_then(|root| github_repo_identifier(Path::new(&root)));
        let mut branches = Vec::new();
        let mut seen_branches = HashSet::new();
        for observed in db
            .list_work_scope_observed_branches(&work_scope)
            .await
            .map_err(|err| err.to_string())?
        {
            if let Some(candidate) = normalize_candidate_for_target_repo(
                repository_identity.as_deref(),
                PrBranchCandidate {
                    repository_identity: observed.repository_identity,
                    branch_name: observed.branch_name,
                },
            ) {
                if seen_branches.insert(candidate.clone()) {
                    branches.push(candidate);
                }
            }
        }
        if let Some(candidate) = candidate_for_conv_branch(&conv).and_then(|candidate| {
            normalize_candidate_for_target_repo(repository_identity.as_deref(), candidate)
        }) {
            if seen_branches.insert(candidate.clone()) {
                branches.push(candidate);
            }
        }
        targets.push(PollTarget {
            work_scope,
            worktree_path: PathBuf::from(worktree_path),
            repository_identity,
            candidate_branches: branches,
        });
    }

    Ok(targets)
}

async fn poll_target(manager: &Arc<RuntimeManager>, target: PollTarget) {
    if !target.worktree_path.is_dir() {
        return;
    }

    let worktree_path = target.worktree_path.clone();
    let repository_identity = target.repository_identity.clone();
    let candidate_branches = target.candidate_branches.clone();
    let refreshes = match tokio::task::spawn_blocking(move || {
        candidate_branches
            .into_iter()
            .map(|candidate| {
                let refresh = crate::api::pr_monitoring::get_pr_status_for_repo_branch(
                    &worktree_path,
                    &candidate.repository_identity,
                    &candidate.branch_name,
                );
                (candidate, refresh)
            })
            .collect::<Vec<_>>()
    })
    .await
    {
        Ok(refreshes) => refreshes,
        Err(err) => {
            tracing::debug!(work_scope = %target.work_scope, error = %err, "background PR status poll task failed");
            return;
        }
    };

    let observations: Vec<_> = refreshes
        .into_iter()
        .flat_map(|(candidate, refresh)| {
            let keep_candidate = repository_identity
                .as_deref()
                .is_none_or(|repo| repo == candidate.repository_identity);
            if !keep_candidate {
                return Vec::new();
            }
            observations_for_target_repo(repository_identity.as_deref(), refresh.observations)
        })
        .collect();
    let latest_durable_branch = target.candidate_branches.iter().find_map(|candidate| {
        repository_identity
            .as_deref()
            .is_none_or(|repo| repo == candidate.repository_identity)
            .then(
                || phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                    repository_identity: candidate.repository_identity.clone(),
                    branch_name: candidate.branch_name.clone(),
                },
            )
    });

    let persist_ok = if observations.is_empty() {
        true
    } else {
        match manager
            .db()
            .upsert_work_scope_pr_observations(&target.work_scope, &observations)
            .await
        {
            Ok(_) => true,
            Err(err) => {
                tracing::debug!(work_scope = %target.work_scope, error = %err, "background PR status poll could not persist observations");
                false
            }
        }
    };

    if persist_ok {
        if let Err(err) = manager
            .db()
            .derive_active_work_scope_pr_selection(
                &target.work_scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: latest_durable_branch,
                },
                None,
            )
            .await
        {
            tracing::debug!(work_scope = %target.work_scope, error = %err, "background PR status poll could not derive active selection");
            return;
        }
        manager
            .broadcast_work_scope_update(&target.work_scope)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_interval_stays_within_bounds() {
        assert_eq!(
            interval_with_jitter(Duration::from_secs(300), Duration::from_secs(90), 0,),
            Duration::from_secs(210),
        );
        assert_eq!(
            interval_with_jitter(Duration::from_secs(300), Duration::from_secs(90), 90,),
            Duration::from_secs(300),
        );
        assert_eq!(
            interval_with_jitter(Duration::from_secs(300), Duration::from_secs(90), 180,),
            Duration::from_secs(390),
        );
        assert_eq!(
            interval_with_jitter(Duration::from_secs(300), Duration::from_secs(90), 900,),
            Duration::from_secs(390),
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn normalize_candidate_for_target_repo_rejects_cross_repo_branch() {
        let kept = normalize_candidate_for_target_repo(
            Some("acme/repo"),
            PrBranchCandidate {
                repository_identity: "fork/repo".to_string(),
                branch_name: "feature".to_string(),
            },
        );
        assert!(kept.is_none());
    }

    #[test]
    fn normalize_candidate_for_target_repo_maps_local_path_identity_to_github_slug() {
        let kept = normalize_candidate_for_target_repo(
            Some("acme/repo"),
            PrBranchCandidate {
                repository_identity: "/tmp/local/repo".to_string(),
                branch_name: "feature".to_string(),
            },
        )
        .expect("candidate");
        assert_eq!(kept.repository_identity, "acme/repo");
        assert_eq!(kept.branch_name, "feature");
    }

    #[test]
    fn inference_input_uses_full_candidate_identity() {
        let input = inference_input_from_candidates(
            Some("acme/repo"),
            &[
                PrBranchCandidate {
                    repository_identity: "fork/repo".to_string(),
                    branch_name: "feature".to_string(),
                },
                PrBranchCandidate {
                    repository_identity: "acme/repo".to_string(),
                    branch_name: "feature".to_string(),
                },
            ],
        );
        let branch = input.latest_observed_branch.expect("candidate");
        assert_eq!(branch.repository_identity, "acme/repo");
        assert_eq!(branch.branch_name, "feature");
    }

    #[test]
    fn observations_for_target_repo_filters_cross_repo_prs() {
        let observations = observations_for_target_repo(
            Some("acme/repo"),
            vec![
                crate::db::WorkScopePrObservation {
                    repo_owner: "fork".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 1,
                    title: "fork".to_string(),
                    url: "https://example.test/fork/repo/1".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: crate::api::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "feature".to_string(),
                    github_updated_at: None,
                },
                crate::db::WorkScopePrObservation {
                    repo_owner: "acme".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 2,
                    title: "upstream".to_string(),
                    url: "https://example.test/acme/repo/2".to_string(),
                    state: "OPEN".to_string(),
                    draft: false,
                    display_state: crate::api::PrDisplayState::Open,
                    base: "main".to_string(),
                    head: "feature".to_string(),
                    github_updated_at: None,
                },
            ],
        );
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].repo_owner, "acme");
        assert_eq!(observations[0].pr_number, 2);
    }
}
