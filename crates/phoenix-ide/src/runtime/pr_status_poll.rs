use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt as _;

use super::RuntimeManager;
use crate::db::{ConvMode, Database};
use crate::work_scope::WorkScope;

pub(crate) const POLL_BASE_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const POLL_JITTER: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PollTarget {
    work_scope: WorkScope,
    worktree_path: PathBuf,
    candidate_branches: Vec<String>,
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
        let (branch_name, worktree_path) = match &conv.conv_mode {
            ConvMode::Work {
                branch_name,
                worktree_path,
                ..
            }
            | ConvMode::Branch {
                branch_name,
                worktree_path,
                ..
            } => (branch_name.to_string(), worktree_path.to_string()),
            _ => continue,
        };
        let work_scope = WorkScope::resolve(&conv.id, Some(Path::new(&worktree_path)));
        if !seen.insert(work_scope.clone()) {
            continue;
        }
        let mut branches = BTreeSet::new();
        branches.insert(branch_name);
        for observed in db
            .list_work_scope_observed_branches(&work_scope)
            .await
            .map_err(|err| err.to_string())?
        {
            branches.insert(observed.branch_name);
        }
        targets.push(PollTarget {
            work_scope,
            worktree_path: PathBuf::from(worktree_path),
            candidate_branches: branches.into_iter().collect(),
        });
    }

    Ok(targets)
}

async fn poll_target(manager: &Arc<RuntimeManager>, target: PollTarget) {
    if !target.worktree_path.is_dir() {
        return;
    }

    let worktree_path = target.worktree_path.clone();
    let candidate_branches = target.candidate_branches.clone();
    let refreshes = match tokio::task::spawn_blocking(move || {
        candidate_branches
            .into_iter()
            .map(|branch_name| {
                crate::api::pr_monitoring::get_pr_status_for_branch(&worktree_path, &branch_name)
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
        .flat_map(|refresh| refresh.observations)
        .collect();
    if observations.is_empty() {
        return;
    }

    match manager
        .db()
        .upsert_work_scope_pr_observations(&target.work_scope, &observations)
        .await
    {
        Ok(_) => {
            manager
                .broadcast_work_scope_update(&target.work_scope)
                .await;
        }
        Err(err) => {
            tracing::debug!(work_scope = %target.work_scope, error = %err, "background PR status poll could not persist observations");
        }
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
