use crate::{Database, DbError, DbResult};
use phoenix_core::git_repository::GitRepositoryId;
use phoenix_core::work_scope::WorkScopeId;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub(crate) struct DormantGitRepositoryCatchupAuthorityState {
    current_marker: Mutex<Option<Arc<LegacyWriterExclusionMarker>>>,
}

#[derive(Debug)]
struct LegacyWriterExclusionMarker;

#[derive(Debug, Clone)]
pub(crate) struct DormantGitRepositoryTargetBinding {
    state: Arc<DormantGitRepositoryCatchupAuthorityState>,
}

impl DormantGitRepositoryTargetBinding {
    pub(crate) fn for_state(state: Arc<DormantGitRepositoryCatchupAuthorityState>) -> Self {
        Self { state }
    }

    fn points_to(&self, state: &Arc<DormantGitRepositoryCatchupAuthorityState>) -> bool {
        Arc::ptr_eq(&self.state, state)
    }
}

#[derive(Debug)]
pub(crate) struct DormantGitRepositoryCatchupPermit {
    target: Arc<DormantGitRepositoryCatchupAuthorityState>,
    marker: Arc<LegacyWriterExclusionMarker>,
}

#[derive(Debug)]
pub(crate) struct DormantGitRepositoryCatchupReceipt {
    target: Arc<DormantGitRepositoryCatchupAuthorityState>,
    marker: Arc<LegacyWriterExclusionMarker>,
}

impl DormantGitRepositoryCatchupReceipt {
    #[cfg(test)]
    fn same_operation_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.target, &other.target) && Arc::ptr_eq(&self.marker, &other.marker)
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct TestDormantGitRepositoryExclusionProof {
    marker: Arc<LegacyWriterExclusionMarker>,
}

#[cfg(test)]
impl TestDormantGitRepositoryExclusionProof {
    fn new() -> Self {
        Self {
            marker: Arc::new(LegacyWriterExclusionMarker),
        }
    }
}

impl DormantGitRepositoryCatchupPermit {
    #[cfg(test)]
    fn test_only_mint(db: &Database, proof: TestDormantGitRepositoryExclusionProof) -> Self {
        db.install_dormant_git_repository_catchup_marker(proof.marker.clone());
        Self {
            target: db.dormant_git_repository_target_binding().state,
            marker: proof.marker,
        }
    }
}

impl Database {
    fn install_dormant_git_repository_catchup_marker(
        &self,
        marker: Arc<LegacyWriterExclusionMarker>,
    ) {
        let mut current_marker = self
            .dormant_git_repository_catchup_authority_state
            .current_marker
            .lock()
            .expect("catch-up authority mutex poisoned");
        *current_marker = Some(marker);
    }

    fn lock_dormant_git_repository_catchup_marker(
        &self,
    ) -> std::sync::MutexGuard<'_, Option<Arc<LegacyWriterExclusionMarker>>> {
        self.dormant_git_repository_catchup_authority_state
            .current_marker
            .lock()
            .expect("catch-up authority mutex poisoned")
    }
}

#[derive(Debug)]
pub(crate) struct DormantGitRepositoryCatchupOutcome {
    pub stats: DormantGitRepositoryCatchupStats,
    pub receipt: DormantGitRepositoryCatchupReceipt,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DormantGitRepositoryCatchupStats {
    pub inserted_git_repositories: usize,
    pub deleted_git_repositories: usize,
    pub inserted_work_scope_attachments: usize,
    pub replaced_work_scope_attachments: usize,
    pub deleted_work_scope_attachments: usize,
    pub deleted_locator_observations: usize,
    pub deleted_default_branch_observations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedAttachment {
    work_scope_id: WorkScopeId,
    repository_id: GitRepositoryId,
}

pub(crate) async fn catch_up_dormant_git_repositories(
    db: &Database,
    permit: DormantGitRepositoryCatchupPermit,
) -> DbResult<DormantGitRepositoryCatchupOutcome> {
    let database_target = db.dormant_git_repository_target_binding();
    if !database_target.points_to(&permit.target) {
        return Err(DbError::DormantGitRepositoryCatchupPermitTargetMismatch);
    }

    {
        let mut current_marker = db.lock_dormant_git_repository_catchup_marker();
        let matches_current = current_marker
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &permit.marker));
        if !matches_current {
            return Err(DbError::DormantGitRepositoryCatchupStaleOperation);
        }
        current_marker.take();
    }

    let receipt = DormantGitRepositoryCatchupReceipt {
        target: permit.target,
        marker: permit.marker,
    };
    let mut tx = db.pool().begin().await?;
    let result = catch_up_dormant_git_repositories_tx(&mut tx).await;
    match result {
        Ok(stats) => {
            tx.commit().await?;
            Ok(DormantGitRepositoryCatchupOutcome { stats, receipt })
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

async fn catch_up_dormant_git_repositories_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<DormantGitRepositoryCatchupStats> {
    let expected_repository_ids = load_expected_repositories(tx).await?;
    let expected_scope_attachments = load_expected_scope_attachments(tx).await?;
    let actual_repository_ids = load_actual_repository_ids(tx).await?;
    let actual_attachments = load_actual_attachments(tx).await?;
    let expected_attachments = resolve_expected_scope_attachments(expected_scope_attachments)?;

    let mut stats = DormantGitRepositoryCatchupStats::default();
    delete_all_repository_observations_tx(tx, &mut stats).await?;

    for repository_id in expected_repository_ids.difference(&actual_repository_ids) {
        sqlx::query("INSERT INTO git_repositories (id) VALUES (?1)")
            .bind(repository_id.as_str())
            .execute(&mut **tx)
            .await?;
        stats.inserted_git_repositories += 1;
    }

    for (work_scope_id, expected_repository_id) in &expected_attachments {
        match actual_attachments.get(work_scope_id) {
            None => {
                sqlx::query(
                    "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
                     VALUES (?1, ?2)",
                )
                .bind(work_scope_id.as_str())
                .bind(expected_repository_id.as_str())
                .execute(&mut **tx)
                .await?;
                stats.inserted_work_scope_attachments += 1;
            }
            Some(actual_repository_id) if actual_repository_id == expected_repository_id => {}
            Some(_) => {
                sqlx::query(
                    "UPDATE work_scope_git_repositories
                     SET repository_id = ?2
                     WHERE work_scope_id = ?1",
                )
                .bind(work_scope_id.as_str())
                .bind(expected_repository_id.as_str())
                .execute(&mut **tx)
                .await?;
                stats.replaced_work_scope_attachments += 1;
            }
        }
    }

    for (work_scope_id, actual_repository_id) in &actual_attachments {
        if expected_attachments.contains_key(work_scope_id) {
            continue;
        }
        let _ = actual_repository_id;
        sqlx::query("DELETE FROM work_scope_git_repositories WHERE work_scope_id = ?1")
            .bind(work_scope_id.as_str())
            .execute(&mut **tx)
            .await?;
        stats.deleted_work_scope_attachments += 1;
    }

    let repository_ids_with_attachments = load_attached_repository_ids(tx).await?;
    for repository_id in actual_repository_ids.difference(&expected_repository_ids) {
        if repository_ids_with_attachments.contains(repository_id) {
            continue;
        }
        sqlx::query("DELETE FROM git_repositories WHERE id = ?1")
            .bind(repository_id.as_str())
            .execute(&mut **tx)
            .await?;
        stats.deleted_git_repositories += 1;
    }

    Ok(stats)
}

async fn load_expected_repositories(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<BTreeSet<GitRepositoryId>> {
    let rows = sqlx::query("SELECT id FROM projects ORDER BY id")
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|row| parse_git_repository_id(row.get::<String, _>("id")))
        .collect()
}

async fn load_actual_repository_ids(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<BTreeSet<GitRepositoryId>> {
    let rows = sqlx::query("SELECT id FROM git_repositories ORDER BY id")
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|row| parse_git_repository_id(row.get::<String, _>("id")))
        .collect()
}

async fn load_attached_repository_ids(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<BTreeSet<GitRepositoryId>> {
    let rows = sqlx::query(
        "SELECT DISTINCT repository_id FROM work_scope_git_repositories ORDER BY repository_id",
    )
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| parse_git_repository_id(row.get::<String, _>("repository_id")))
        .collect()
}

async fn load_expected_scope_attachments(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<Vec<ExpectedAttachment>> {
    let rows = sqlx::query(
        "SELECT attachment.work_scope_id, c.project_id
         FROM conversation_work_scope_attachments attachment
         JOIN conversations c ON c.id = attachment.conversation_id
         ORDER BY attachment.work_scope_id, c.project_id, attachment.conversation_id",
    )
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .filter_map(|row| {
            row.get::<Option<String>, _>("project_id")
                .map(|project_id| {
                    Ok(ExpectedAttachment {
                        work_scope_id: parse_work_scope_id(row.get::<String, _>("work_scope_id"))?,
                        repository_id: parse_git_repository_id(project_id)?,
                    })
                })
        })
        .collect()
}

fn resolve_expected_scope_attachments(
    attachments: Vec<ExpectedAttachment>,
) -> DbResult<BTreeMap<WorkScopeId, GitRepositoryId>> {
    let mut grouped: BTreeMap<WorkScopeId, Vec<GitRepositoryId>> = BTreeMap::new();
    for attachment in attachments {
        grouped
            .entry(attachment.work_scope_id)
            .or_default()
            .push(attachment.repository_id);
    }

    let mut expected = BTreeMap::new();
    for (work_scope_id, repository_ids) in grouped {
        let mut distinct_repository_ids = repository_ids;
        distinct_repository_ids.sort();
        distinct_repository_ids.dedup();
        if distinct_repository_ids.len() > 1 {
            return Err(DbError::GitRepositoryWorkScopeProjectConflict {
                work_scope_id,
                repository_ids: distinct_repository_ids,
            });
        }
        if let Some(repository_id) = distinct_repository_ids.into_iter().next() {
            expected.insert(work_scope_id, repository_id);
        }
    }
    Ok(expected)
}

async fn load_actual_attachments(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<BTreeMap<WorkScopeId, GitRepositoryId>> {
    let rows = sqlx::query(
        "SELECT work_scope_id, repository_id
         FROM work_scope_git_repositories
         ORDER BY work_scope_id",
    )
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok((
                parse_work_scope_id(row.get::<String, _>("work_scope_id"))?,
                parse_git_repository_id(row.get::<String, _>("repository_id"))?,
            ))
        })
        .collect()
}

async fn delete_all_repository_observations_tx(
    tx: &mut Transaction<'_, Sqlite>,
    stats: &mut DormantGitRepositoryCatchupStats,
) -> DbResult<()> {
    let deleted_locator_observations =
        sqlx::query("DELETE FROM git_repository_locator_observations")
            .execute(&mut **tx)
            .await?
            .rows_affected();
    stats.deleted_locator_observations +=
        usize::try_from(deleted_locator_observations).map_err(|_| {
            DbError::Serialization("deleted locator row count exceeds usize".to_string())
        })?;

    let deleted_default_branch_observations =
        sqlx::query("DELETE FROM git_repository_default_branch_observations")
            .execute(&mut **tx)
            .await?
            .rows_affected();
    stats.deleted_default_branch_observations +=
        usize::try_from(deleted_default_branch_observations).map_err(|_| {
            DbError::Serialization("deleted default branch row count exceeds usize".to_string())
        })?;

    Ok(())
}

fn parse_work_scope_id(value: String) -> DbResult<WorkScopeId> {
    WorkScopeId::parse(value).map_err(|error| DbError::Serialization(error.to_string()))
}

fn parse_git_repository_id(value: String) -> DbResult<GitRepositoryId> {
    GitRepositoryId::parse(value).map_err(|error| DbError::Serialization(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{run_pending_migrations, ConvMode, Database};
    use chrono::Utc;
    use phoenix_core::llm_language::LlmLanguage;
    use phoenix_core::work_scope::{AuthorityKind, EnvironmentContext};

    #[tokio::test]
    async fn catchup_inserts_missing_git_repository() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-a").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-a".to_string()]);
    }

    #[tokio::test]
    async fn catchup_inserts_missing_git_repository_on_file_backed_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catchup.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        insert_project(&db, "repo-file").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-file".to_string()]);
    }

    #[tokio::test]
    async fn catchup_replaces_superseded_attachment() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-good").await;
        insert_git_repository(&db, "repo-good").await;
        insert_git_repository(&db, "repo-stale").await;
        let scope = insert_scope(&db, "scope-replace").await;
        insert_conversation_with_scope_and_project(&db, "conv-replace", &scope, Some("repo-good"))
            .await;
        insert_attachment(&db, &scope, "repo-stale").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.replaced_work_scope_attachments, 1);
        assert_eq!(
            attachment_rows(&db).await,
            vec![(scope.as_str().to_string(), "repo-good".to_string())]
        );
    }

    #[tokio::test]
    async fn catchup_removes_deleted_source_project_attachment_and_observations() {
        let db = Database::open_in_memory().await.unwrap();
        insert_git_repository(&db, "repo-gone").await;
        insert_locator_observation(&db, "repo-gone").await;
        insert_default_branch_observation(&db, "repo-gone").await;
        let scope = insert_scope(&db, "scope-delete").await;
        insert_attachment(&db, &scope, "repo-gone").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.deleted_work_scope_attachments, 1);
        assert_eq!(stats.deleted_git_repositories, 1);
        assert_eq!(stats.deleted_locator_observations, 1);
        assert_eq!(stats.deleted_default_branch_observations, 1);
        assert!(attachment_rows(&db).await.is_empty());
        assert!(repository_ids(&db).await.is_empty());
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn catchup_is_idempotent_for_exact_set() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-a").await;
        insert_project(&db, "repo-b").await;
        insert_git_repository(&db, "repo-a").await;
        insert_git_repository(&db, "repo-b").await;
        let scope_a = insert_scope(&db, "scope-a").await;
        let scope_b = insert_scope(&db, "scope-b").await;
        insert_conversation_with_scope_and_project(&db, "conv-a", &scope_a, Some("repo-a")).await;
        insert_conversation_with_scope_and_project(&db, "conv-b", &scope_b, Some("repo-b")).await;
        insert_attachment(&db, &scope_a, "repo-a").await;
        insert_attachment(&db, &scope_b, "repo-b").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats, DormantGitRepositoryCatchupStats::default());
        assert_eq!(
            repository_ids(&db).await,
            vec!["repo-a".to_string(), "repo-b".to_string()]
        );
        assert_eq!(attachment_rows(&db).await.len(), 2);
    }

    #[tokio::test]
    async fn catchup_rolls_back_transaction_on_multi_project_conflict() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-a").await;
        insert_project(&db, "repo-b").await;
        insert_git_repository(&db, "repo-stale").await;
        insert_locator_observation(&db, "repo-stale").await;
        insert_default_branch_observation(&db, "repo-stale").await;
        let scope = insert_scope(&db, "scope-conflict").await;
        insert_conversation_with_scope_and_project(&db, "conv-1", &scope, Some("repo-a")).await;
        insert_conversation_with_scope_and_project(&db, "conv-2", &scope, Some("repo-b")).await;
        insert_attachment(&db, &scope, "repo-stale").await;

        let err = run_catchup(&db).await.unwrap_err();

        match err {
            DbError::GitRepositoryWorkScopeProjectConflict {
                work_scope_id,
                repository_ids,
            } => {
                assert_eq!(work_scope_id, scope);
                assert_eq!(
                    repository_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    vec!["repo-a", "repo-b"]
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }

        assert_eq!(repository_ids(&db).await, vec!["repo-stale".to_string()]);
        assert_eq!(
            attachment_rows(&db).await,
            vec![(scope.as_str().to_string(), "repo-stale".to_string())]
        );
        assert_eq!(count_locator_rows(&db).await, 1);
        assert_eq!(count_default_branch_rows(&db).await, 1);
    }

    #[tokio::test]
    async fn catchup_reports_conflict_for_opaque_repository_ids_with_commas_and_padding() {
        let db = Database::open_in_memory().await.unwrap();
        let repo_a = "repo,one";
        let repo_b = "  repo-two  ";
        insert_project(&db, repo_a).await;
        insert_project(&db, repo_b).await;
        let scope = insert_scope(&db, "scope-opaque-conflict").await;
        insert_conversation_with_scope_and_project(&db, "conv-opaque-1", &scope, Some(repo_a))
            .await;
        insert_conversation_with_scope_and_project(&db, "conv-opaque-2", &scope, Some(repo_b))
            .await;

        let err = run_catchup(&db).await.unwrap_err();

        match err {
            DbError::GitRepositoryWorkScopeProjectConflict {
                work_scope_id,
                repository_ids,
            } => {
                assert_eq!(work_scope_id, scope);
                assert_eq!(
                    repository_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    vec![repo_b.to_string(), repo_a.to_string()]
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn catchup_deletes_all_observations_even_for_retained_attached_repo() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-keep").await;
        insert_git_repository(&db, "repo-keep").await;
        let scope = insert_scope(&db, "scope-keep").await;
        insert_conversation_with_scope_and_project(&db, "conv-keep", &scope, Some("repo-keep"))
            .await;
        insert_attachment(&db, &scope, "repo-keep").await;
        insert_locator_observation(&db, "repo-keep").await;
        insert_default_branch_observation(&db, "repo-keep").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.deleted_locator_observations, 1);
        assert_eq!(stats.deleted_default_branch_observations, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-keep".to_string()]);
        assert_eq!(
            attachment_rows(&db).await,
            vec![(scope.as_str().to_string(), "repo-keep".to_string())]
        );
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn catchup_deletes_attachment_when_scope_has_zero_projects() {
        let db = Database::open_in_memory().await.unwrap();
        insert_git_repository(&db, "repo-gone").await;
        let scope = insert_scope(&db, "scope-zero").await;
        insert_conversation_with_scope_and_project(&db, "conv-zero", &scope, None).await;
        insert_attachment(&db, &scope, "repo-gone").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.deleted_work_scope_attachments, 1);
        assert!(attachment_rows(&db).await.is_empty());
        assert!(repository_ids(&db).await.is_empty());
    }

    #[tokio::test]
    async fn cloned_database_handles_share_authority_identity() {
        let db = Database::open_in_memory().await.unwrap();
        let cloned = db.clone();
        insert_project(&db, "repo-shared").await;
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(
            &db,
            TestDormantGitRepositoryExclusionProof::new(),
        );

        let outcome = cloned
            .catch_up_dormant_git_repositories(permit)
            .await
            .unwrap();

        assert_eq!(outcome.stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-shared".to_string()]);
    }

    #[tokio::test]
    async fn catchup_rejects_different_in_memory_database_before_mutation_and_leaves_rows_unchanged(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        let other_db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-a").await;
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(
            &other_db,
            TestDormantGitRepositoryExclusionProof::new(),
        );

        let error = db
            .catch_up_dormant_git_repositories(permit)
            .await
            .unwrap_err();

        assert_target_mismatch(error);
        assert!(repository_ids(&db).await.is_empty());
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn catchup_rejects_independently_reopened_file_database_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reopened-catchup.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        run_pending_migrations(reopened.pool()).await.unwrap();
        insert_project(&db, "repo-a").await;
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(
            &reopened,
            TestDormantGitRepositoryExclusionProof::new(),
        );

        let error = db
            .catch_up_dormant_git_repositories(permit)
            .await
            .unwrap_err();

        assert_target_mismatch(error);
        assert!(repository_ids(&db).await.is_empty());
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn newer_mint_supersedes_older_and_stale_rejection_leaves_newer_usable() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-b").await;
        let permit_a = DormantGitRepositoryCatchupPermit::test_only_mint(
            &db,
            TestDormantGitRepositoryExclusionProof::new(),
        );
        let permit_b = DormantGitRepositoryCatchupPermit::test_only_mint(
            &db,
            TestDormantGitRepositoryExclusionProof::new(),
        );

        let stale = db
            .catch_up_dormant_git_repositories(permit_a)
            .await
            .unwrap_err();
        assert_stale_operation(stale);
        assert!(repository_ids(&db).await.is_empty());

        let outcome = db
            .catch_up_dormant_git_repositories(permit_b)
            .await
            .unwrap();
        assert_eq!(outcome.stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-b".to_string()]);
    }

    #[tokio::test]
    async fn distinct_operation_permits_produce_non_substitutable_receipts() {
        let db = Database::open_in_memory().await.unwrap();
        let first = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let second = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();

        assert!(!first.receipt.same_operation_as(&second.receipt));
        assert_eq!(first.stats, DormantGitRepositoryCatchupStats::default());
        assert_eq!(second.stats, DormantGitRepositoryCatchupStats::default());
    }

    #[tokio::test]
    async fn successful_catchup_spends_operation_and_another_permit_for_same_marker_goes_stale() {
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-spent").await;
        let proof = TestDormantGitRepositoryExclusionProof::new();
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(&db, proof.clone());
        let stale_retry = DormantGitRepositoryCatchupPermit::test_only_mint(&db, proof);

        let outcome = db.catch_up_dormant_git_repositories(permit).await.unwrap();
        assert_eq!(outcome.stats.inserted_git_repositories, 1);

        let stale = db
            .catch_up_dormant_git_repositories(stale_retry)
            .await
            .unwrap_err();
        assert_stale_operation(stale);
    }

    #[tokio::test]
    async fn begin_failure_spends_operation_without_restoring_marker() {
        let db = Database::open_in_memory().await.unwrap();
        let proof = TestDormantGitRepositoryExclusionProof::new();
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(&db, proof.clone());
        let stale_retry = DormantGitRepositoryCatchupPermit::test_only_mint(&db, proof);
        db.pool().close().await;

        let begin_error = db
            .catch_up_dormant_git_repositories(permit)
            .await
            .unwrap_err();
        assert!(matches!(begin_error, DbError::Sqlx(_)));

        let stale = db
            .catch_up_dormant_git_repositories(stale_retry)
            .await
            .unwrap_err();
        assert_stale_operation(stale);
    }

    #[tokio::test]
    async fn permit_consumption_prevents_reuse_by_move() {
        let db = Database::open_in_memory().await.unwrap();
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(
            &db,
            TestDormantGitRepositoryExclusionProof::new(),
        );
        let consume = |_: DormantGitRepositoryCatchupPermit| {};
        consume(permit);
    }

    async fn run_catchup(db: &Database) -> DbResult<DormantGitRepositoryCatchupStats> {
        let outcome =
            run_catchup_with_proof(db, TestDormantGitRepositoryExclusionProof::new()).await?;
        Ok(outcome.stats)
    }

    async fn run_catchup_with_proof(
        db: &Database,
        proof: TestDormantGitRepositoryExclusionProof,
    ) -> DbResult<DormantGitRepositoryCatchupOutcome> {
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(db, proof);
        db.catch_up_dormant_git_repositories(permit).await
    }

    async fn insert_project(db: &Database, id: &str) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at)
             VALUES (?1, ?2, 'main', ?3)",
        )
        .bind(id)
        .bind(format!("/tmp/{id}"))
        .bind(now.to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn insert_git_repository(db: &Database, id: &str) {
        sqlx::query("INSERT INTO git_repositories (id) VALUES (?1)")
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn insert_scope(db: &Database, raw: &str) -> WorkScopeId {
        let scope = WorkScopeId::parse(raw).unwrap();
        let now = Utc::now().to_rfc3339();
        let mut tx = db.pool().begin().await.unwrap();
        Database::insert_work_scope_tx(
            &mut tx,
            &scope,
            AuthorityKind::RestrictedExplore,
            EnvironmentContext::None,
            &now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        scope
    }

    async fn insert_conversation_with_scope_and_project(
        db: &Database,
        id: &str,
        scope: &WorkScopeId,
        project_id: Option<&str>,
    ) {
        let slug = format!("slug-{id}");
        db.create_conversation_with_project(
            id,
            &slug,
            "/tmp",
            true,
            None,
            None,
            project_id,
            &ConvMode::Direct,
            None,
            None,
            None,
            LlmLanguage::default(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = ?2")
            .bind(scope.as_str())
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn insert_attachment(db: &Database, scope: &WorkScopeId, repository_id: &str) {
        sqlx::query(
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
             VALUES (?1, ?2)",
        )
        .bind(scope.as_str())
        .bind(repository_id)
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn insert_locator_observation(db: &Database, repository_id: &str) {
        sqlx::query(
            "INSERT INTO git_repository_locator_observations (
                repository_id, locator_kind, status, path, observed_at
             ) VALUES (?1, 'common_dir', 'present', '/tmp/common', ?2)",
        )
        .bind(repository_id)
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn insert_default_branch_observation(db: &Database, repository_id: &str) {
        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                repository_id, generation, status, branch, provenance, observed_at
             ) VALUES (?1, 0, 'resolved', 'main', 'user_selected', ?2)",
        )
        .bind(repository_id)
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn repository_ids(db: &Database) -> Vec<String> {
        sqlx::query_scalar("SELECT id FROM git_repositories ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap()
    }

    async fn attachment_rows(db: &Database) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT work_scope_id, repository_id
             FROM work_scope_git_repositories
             ORDER BY work_scope_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
    }

    async fn count_locator_rows(db: &Database) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_locator_observations")
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    async fn count_default_branch_rows(db: &Database) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_default_branch_observations")
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    fn assert_target_mismatch(error: DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryCatchupPermitTargetMismatch
        ));
    }

    fn assert_stale_operation(error: DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryCatchupStaleOperation
        ));
    }

    #[test]
    fn api_shape_requires_database_and_proof_and_keeps_types_private_to_module() {
        let _mint: fn(
            &Database,
            TestDormantGitRepositoryExclusionProof,
        ) -> DormantGitRepositoryCatchupPermit = DormantGitRepositoryCatchupPermit::test_only_mint;
        let _proof_new: fn() -> TestDormantGitRepositoryExclusionProof =
            TestDormantGitRepositoryExclusionProof::new;
        let _target_for_state: fn(
            std::sync::Arc<DormantGitRepositoryCatchupAuthorityState>,
        ) -> DormantGitRepositoryTargetBinding = DormantGitRepositoryTargetBinding::for_state;
    }

    #[test]
    fn expected_attachment_type_uses_typed_ids() {
        let attachment = ExpectedAttachment {
            work_scope_id: WorkScopeId::parse("scope-typed").unwrap(),
            repository_id: GitRepositoryId::parse("repo-typed").unwrap(),
        };
        assert_eq!(attachment.work_scope_id.as_str(), "scope-typed");
        assert_eq!(attachment.repository_id.as_str(), "repo-typed");
    }
}
