use crate::{Database, DbError, DbResult, ProjectSeedId};
use phoenix_core::git_repository::GitRepositoryId;
use phoenix_core::work_scope::WorkScopeId;
use sqlx::{Connection, Executor, Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const READINESS_SAMPLE_LIMIT: usize = 10;
const READINESS_SAMPLE_BYTE_LIMIT: usize = 512;

#[derive(Debug, Default)]
pub(crate) struct DormantGitRepositoryCatchupAuthorityState {
    lifecycle: Mutex<DormantGitRepositoryLifecycle>,
}

#[derive(Debug, Default)]
struct DormantGitRepositoryLifecycle {
    current_marker: Option<Arc<LegacyWriterExclusionMarker>>,
    catchup_in_progress: Option<Arc<LegacyWriterExclusionMarker>>,
    latest_completed_marker: Option<Arc<LegacyWriterExclusionMarker>>,
    readiness_claim: Option<Arc<LegacyWriterExclusionMarker>>,
    consumed_readiness_marker: Option<Arc<LegacyWriterExclusionMarker>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(test, serde(rename_all = "snake_case"))]
pub(crate) enum DormantGitRepositoryR1Eligibility {
    Eligible,
    Ineligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(test, serde(rename_all = "snake_case"))]
pub(crate) enum DormantGitRepositoryReadinessDiagnosticCategory {
    MissingRepository,
    UnexpectedRepository,
    ConflictingScopeAttachment,
    MissingScopeAttachment,
    MismatchedScopeAttachment,
    UnexpectedScopeAttachment,
    MigrationLedger,
    Schema,
    ForeignKey,
    UnexpectedLocatorObservation,
    UnexpectedDefaultBranchObservation,
    CandidateBuildUnavailable,
    CandidateBuildDirty,
}

#[derive(Debug, Default)]
struct DormantGitRepositoryReadinessDiagnosticSamples {
    total_count: usize,
    samples: Vec<String>,
}

#[derive(Debug, Default)]
struct DormantGitRepositoryReadinessDiagnostics {
    categories: BTreeMap<
        DormantGitRepositoryReadinessDiagnosticCategory,
        DormantGitRepositoryReadinessDiagnosticSamples,
    >,
    valid_absences: BTreeSet<DormantGitRepositoryValidAbsence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(test, serde(rename_all = "snake_case"))]
pub(crate) enum DormantGitRepositoryValidAbsence {
    LocatorObservationRows,
    DefaultBranchObservationRows,
}

impl DormantGitRepositoryReadinessDiagnostics {
    fn push(
        &mut self,
        category: DormantGitRepositoryReadinessDiagnosticCategory,
        detail: impl AsRef<str>,
    ) {
        let category_samples = self.categories.entry(category).or_default();
        category_samples.total_count += 1;
        if category_samples.samples.len() < READINESS_SAMPLE_LIMIT {
            category_samples.samples.push(bound_sample(detail.as_ref()));
        }
    }

    fn record_valid_absence(&mut self, absence: DormantGitRepositoryValidAbsence) {
        self.valid_absences.insert(absence);
    }

    fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }

    fn typed_categories(&self) -> Vec<DormantGitRepositoryReadinessCategoryEvidence> {
        self.categories
            .iter()
            .map(
                |(category, diagnostics)| DormantGitRepositoryReadinessCategoryEvidence {
                    category: *category,
                    total_count: diagnostics.total_count,
                    samples: diagnostics
                        .samples
                        .iter()
                        .cloned()
                        .map(|detail| DormantGitRepositoryReadinessDiagnosticSample { detail })
                        .collect(),
                },
            )
            .collect()
    }

    fn typed_valid_absences(&self) -> Vec<DormantGitRepositoryValidAbsence> {
        self.valid_absences.iter().copied().collect()
    }
}

fn bound_sample(value: &str) -> String {
    if value.len() <= READINESS_SAMPLE_BYTE_LIMIT {
        return value.to_string();
    }
    let mut end = READINESS_SAMPLE_BYTE_LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", value.get(..end).expect("UTF-8 boundary was checked"))
}

#[derive(Debug, PartialEq, Eq)]
enum DormantGitRepositoryBuildIdentity {
    ExactClean {
        sha: String,
        package_version: String,
    },
    Dirty {
        sha: String,
        package_version: String,
    },
    Unavailable {
        package_version: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(test, serde(rename_all = "snake_case"))]
pub(crate) enum DormantGitRepositoryReadinessStorageKind {
    InMemory,
    FileBacked,
}

#[derive(Debug)]
struct DormantGitRepositoryReadinessRunMarker;

/// Opaque proof that one validation run is bound to exactly one database lifecycle
/// and the completed catch-up operation that produced its receipt. Possession only
/// identifies evidence; it confers no subsequent-operation authority.
#[allow(dead_code, reason = "task 59004 consumes the opaque run binding")]
#[derive(Debug)]
pub(crate) struct DormantGitRepositoryReadinessRunRoot {
    database: Arc<DormantGitRepositoryCatchupAuthorityState>,
    operation: Arc<LegacyWriterExclusionMarker>,
    run_marker: Arc<DormantGitRepositoryReadinessRunMarker>,
    // Generated only when validation completes; this opaque value is the durable
    // identity of this particular readiness observation, not a caller claim.
    run_id: uuid::Uuid,
}

#[allow(
    dead_code,
    reason = "task 59004 consumes the aggregate evidence summary"
)]
#[derive(Debug)]
struct DormantGitRepositoryReadinessSchemaSummary {
    compiled_migration_digest: String,
    compiled_migration_count: usize,
    applied_ledger: Vec<(i64, String)>,
    inspected_r1_ddl: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct DormantGitRepositoryReadinessDiagnosticSample {
    detail: String,
}

impl DormantGitRepositoryReadinessDiagnosticSample {
    #[allow(
        dead_code,
        reason = "the dormant cutover consumer inspects bounded details"
    )]
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Debug)]
pub(crate) struct DormantGitRepositoryReadinessCategoryEvidence {
    category: DormantGitRepositoryReadinessDiagnosticCategory,
    total_count: usize,
    samples: Vec<DormantGitRepositoryReadinessDiagnosticSample>,
}

impl DormantGitRepositoryReadinessCategoryEvidence {
    #[allow(
        dead_code,
        reason = "the dormant cutover consumer matches exhaustive categories"
    )]
    pub(crate) fn category(&self) -> DormantGitRepositoryReadinessDiagnosticCategory {
        self.category
    }

    #[allow(
        dead_code,
        reason = "the dormant cutover consumer inspects total mismatch counts"
    )]
    pub(crate) fn total_count(&self) -> usize {
        self.total_count
    }

    #[allow(
        dead_code,
        reason = "the dormant cutover consumer inspects bounded typed samples"
    )]
    pub(crate) fn samples(&self) -> &[DormantGitRepositoryReadinessDiagnosticSample] {
        &self.samples
    }
}

#[allow(
    dead_code,
    reason = "task 59004 consumes the aggregate readiness facade"
)]
#[derive(Debug)]
pub(crate) struct DormantGitRepositoryReadinessSummary {
    eligibility: DormantGitRepositoryR1Eligibility,
    storage_kind: DormantGitRepositoryReadinessStorageKind,
    diagnostic_categories: Vec<DormantGitRepositoryReadinessCategoryEvidence>,
    valid_absences: Vec<DormantGitRepositoryValidAbsence>,
}

impl DormantGitRepositoryReadinessSummary {
    #[allow(dead_code, reason = "task 59004 consumes aggregate readiness storage")]
    pub(crate) fn storage_kind(&self) -> DormantGitRepositoryReadinessStorageKind {
        self.storage_kind
    }

    #[allow(dead_code, reason = "task 59004 consumes aggregate R1 diagnostics")]
    pub(crate) fn diagnostic_categories(&self) -> &[DormantGitRepositoryReadinessCategoryEvidence] {
        &self.diagnostic_categories
    }

    #[allow(dead_code, reason = "task 59004 consumes explicit R1 valid absences")]
    pub(crate) fn valid_absences(&self) -> &[DormantGitRepositoryValidAbsence] {
        &self.valid_absences
    }
}

/// R1 readiness evidence remains opaque outside this module. Its only dormant
/// consumer seam is a reference to the exact run root.
#[derive(Debug)]
pub(crate) struct DormantGitRepositoryCanonicalReadinessEvidence {
    root: DormantGitRepositoryReadinessRunRoot,
    #[allow(
        dead_code,
        reason = "task 59004 consumes opaque evidence as one aggregate"
    )]
    build: DormantGitRepositoryBuildIdentity,
    #[allow(
        dead_code,
        reason = "task 59004 consumes opaque evidence as one aggregate"
    )]
    schema: DormantGitRepositoryReadinessSchemaSummary,
    #[allow(
        dead_code,
        reason = "task 59004 consumes opaque evidence as one aggregate"
    )]
    summary: DormantGitRepositoryReadinessSummary,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum TestReadinessBuild<'a> {
    ExactClean(&'a str),
    Dirty(&'a str),
    Unavailable,
}

#[cfg(test)]
pub(super) fn test_readiness_evidence(
    build: TestReadinessBuild<'_>,
    eligibility: DormantGitRepositoryR1Eligibility,
) -> DormantGitRepositoryCanonicalReadinessEvidence {
    let package_version = "test".to_string();
    let build = match build {
        TestReadinessBuild::ExactClean(sha) => DormantGitRepositoryBuildIdentity::ExactClean {
            sha: sha.to_string(),
            package_version,
        },
        TestReadinessBuild::Dirty(sha) => DormantGitRepositoryBuildIdentity::Dirty {
            sha: sha.to_string(),
            package_version,
        },
        TestReadinessBuild::Unavailable => {
            DormantGitRepositoryBuildIdentity::Unavailable { package_version }
        }
    };
    DormantGitRepositoryCanonicalReadinessEvidence {
        root: DormantGitRepositoryReadinessRunRoot {
            database: Arc::new(DormantGitRepositoryCatchupAuthorityState::default()),
            operation: Arc::new(LegacyWriterExclusionMarker),
            run_marker: Arc::new(DormantGitRepositoryReadinessRunMarker),
            run_id: uuid::Uuid::new_v4(),
        },
        build,
        schema: DormantGitRepositoryReadinessSchemaSummary {
            compiled_migration_digest: String::new(),
            compiled_migration_count: 0,
            applied_ledger: Vec::new(),
            inspected_r1_ddl: BTreeMap::new(),
        },
        summary: DormantGitRepositoryReadinessSummary {
            eligibility,
            storage_kind: DormantGitRepositoryReadinessStorageKind::FileBacked,
            diagnostic_categories: Vec::new(),
            valid_absences: Vec::new(),
        },
    }
}

impl DormantGitRepositoryCanonicalReadinessEvidence {
    pub(super) fn eligible_clean_candidate_sha(&self) -> Option<&str> {
        match (&self.summary.eligibility, &self.build) {
            (
                DormantGitRepositoryR1Eligibility::Eligible,
                DormantGitRepositoryBuildIdentity::ExactClean { sha, .. },
            ) => Some(sha),
            _ => None,
        }
    }

    #[allow(dead_code, reason = "task 59004 binds the dormant readiness facade")]
    pub(crate) fn summary(&self) -> &DormantGitRepositoryReadinessSummary {
        &self.summary
    }

    #[allow(dead_code, reason = "task 59004 receives the opaque readiness root")]
    pub(crate) fn run_root(&self) -> &DormantGitRepositoryReadinessRunRoot {
        &self.root
    }

    #[cfg(test)]
    fn eligibility(&self) -> &DormantGitRepositoryR1Eligibility {
        &self.summary.eligibility
    }
    #[cfg(test)]
    fn diagnostic_categories(&self) -> Vec<DormantGitRepositoryReadinessDiagnosticCategory> {
        self.summary
            .diagnostic_categories()
            .iter()
            .map(DormantGitRepositoryReadinessCategoryEvidence::category)
            .collect()
    }
    #[cfg(test)]
    fn diagnostics(&self) -> Vec<String> {
        self.summary
            .diagnostic_categories()
            .iter()
            .flat_map(|category| {
                category
                    .samples()
                    .iter()
                    .map(|sample| format!("{:?}: {}", category.category(), sample.detail()))
            })
            .collect()
    }
    #[cfg(test)]
    fn has_fresh_root_from(&self, other: &Self) -> bool {
        !Arc::ptr_eq(&self.root.run_marker, &other.root.run_marker)
    }
    #[cfg(test)]
    fn diagnostic_count(&self, category: DormantGitRepositoryReadinessDiagnosticCategory) -> usize {
        self.summary
            .diagnostic_categories()
            .iter()
            .find_map(|evidence| {
                (evidence.category() == category).then_some(evidence.total_count())
            })
            .unwrap_or_default()
    }
    #[cfg(test)]
    fn has_valid_absence(&self, absence: DormantGitRepositoryValidAbsence) -> bool {
        self.summary.valid_absences().contains(&absence)
    }
}

struct DormantGitRepositoryCatchupInProgressGuard {
    state: Arc<DormantGitRepositoryCatchupAuthorityState>,
    marker: Arc<LegacyWriterExclusionMarker>,
}

impl Drop for DormantGitRepositoryCatchupInProgressGuard {
    fn drop(&mut self) {
        let mut lifecycle = self
            .state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned");
        if lifecycle
            .catchup_in_progress
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &self.marker))
        {
            lifecycle.catchup_in_progress = None;
        }
    }
}

struct DormantGitRepositoryReadinessReceiptClaim {
    state: Arc<DormantGitRepositoryCatchupAuthorityState>,
    receipt: DormantGitRepositoryCatchupReceipt,
    finalized: bool,
}

impl DormantGitRepositoryReadinessReceiptClaim {
    fn finalize(mut self) {
        let mut lifecycle = self
            .state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned");
        if lifecycle
            .readiness_claim
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &self.receipt.marker))
        {
            lifecycle.readiness_claim = None;
            lifecycle.consumed_readiness_marker = Some(self.receipt.marker.clone());
        }
        self.finalized = true;
    }
}

impl Drop for DormantGitRepositoryReadinessReceiptClaim {
    fn drop(&mut self) {
        if !self.finalized {
            let mut lifecycle = self
                .state
                .lifecycle
                .lock()
                .expect("lifecycle mutex poisoned");
            if lifecycle
                .readiness_claim
                .as_ref()
                .is_some_and(|marker| Arc::ptr_eq(marker, &self.receipt.marker))
            {
                lifecycle.readiness_claim = None;
            }
        }
    }
}

#[allow(dead_code, reason = "task 59004 invokes readiness through Database")]
pub(crate) async fn validate_dormant_git_repository_readiness(
    db: &Database,
    receipt: DormantGitRepositoryCatchupReceipt,
) -> DbResult<DormantGitRepositoryCanonicalReadinessEvidence> {
    let claim = db.claim_dormant_git_repository_readiness_receipt(receipt)?;
    let mut guard = QueryOnlyConnectionGuard::acquire(db).await?;
    let original_query_only = read_query_only_flag(guard.connection()).await?;
    guard.arm();
    set_query_only_flag(guard.connection(), true).await?;

    let begin = begin_query_only_transaction(guard.connection()).await;
    if begin.is_err() {
        let error = begin.expect_err("checked failed transaction begin");
        let restored = set_query_only_flag(guard.connection(), original_query_only)
            .await
            .is_ok();
        if restored {
            guard.disarm();
        }
        return Err(error);
    }
    let mut tx = begin.expect("checked successful transaction begin");
    let validation = validate_dormant_git_repository_readiness_tx(&mut tx).await;
    let rollback = tx.rollback().await;
    let restore = set_query_only_flag(guard.connection(), original_query_only).await;
    match (validation, rollback, restore) {
        (Ok(inspection), Ok(()), Ok(())) => {
            guard.disarm();
            let DormantGitRepositoryReadinessInspection {
                diagnostics,
                applied_ledger,
                inspected_r1_ddl,
            } = inspection;
            let build = compiled_build_identity();
            let schema = compiled_schema_summary(applied_ledger, inspected_r1_ddl);
            let eligibility = if diagnostics.is_empty()
                && matches!(build, DormantGitRepositoryBuildIdentity::ExactClean { .. })
            {
                DormantGitRepositoryR1Eligibility::Eligible
            } else {
                DormantGitRepositoryR1Eligibility::Ineligible
            };
            let summary = DormantGitRepositoryReadinessSummary {
                eligibility,
                storage_kind: readiness_storage_kind_for_path(&db.path),
                diagnostic_categories: diagnostics.typed_categories(),
                valid_absences: diagnostics.typed_valid_absences(),
            };
            let root = DormantGitRepositoryReadinessRunRoot {
                database: claim.receipt.target.clone(),
                operation: claim.receipt.marker.clone(),
                run_marker: Arc::new(DormantGitRepositoryReadinessRunMarker),
                run_id: uuid::Uuid::new_v4(),
            };
            claim.finalize();
            Ok(DormantGitRepositoryCanonicalReadinessEvidence {
                root,
                build,
                schema,
                summary,
            })
        }
        (Err(error), Ok(()), Ok(())) => {
            guard.disarm();
            Err(error)
        }
        (_, _, Err(error)) => Err(error),
        (_, Err(error), _) => Err(error.into()),
    }
}

async fn begin_query_only_transaction(
    connection: &mut sqlx::SqliteConnection,
) -> DbResult<Transaction<'_, Sqlite>> {
    Ok(connection.begin().await?)
}

async fn read_query_only_flag<'e, E>(executor: E) -> DbResult<bool>
where
    E: Executor<'e, Database = Sqlite>,
{
    Ok(sqlx::query_scalar::<_, i64>("PRAGMA query_only")
        .fetch_one(executor)
        .await?
        != 0)
}

async fn set_query_only_flag<'e, E>(executor: E, enabled: bool) -> DbResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(if enabled {
        "PRAGMA query_only = 1"
    } else {
        "PRAGMA query_only = 0"
    })
    .execute(executor)
    .await?;
    Ok(())
}

struct QueryOnlyConnectionGuard {
    connection: Option<sqlx::pool::PoolConnection<Sqlite>>,
    armed: bool,
}

impl QueryOnlyConnectionGuard {
    async fn acquire(db: &Database) -> DbResult<Self> {
        Ok(Self {
            connection: Some(db.pool().acquire().await?),
            armed: false,
        })
    }
    fn connection(&mut self) -> &mut sqlx::SqliteConnection {
        self.connection
            .as_deref_mut()
            .expect("guard owns connection")
    }
    fn arm(&mut self) {
        self.armed = true;
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for QueryOnlyConnectionGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(connection) = self.connection.as_mut() {
                connection.close_on_drop();
            }
        }
    }
}

fn compiled_build_identity() -> DormantGitRepositoryBuildIdentity {
    build_identity_from(
        env!("PHOENIX_DB_GIT_SHA"),
        env!("PHOENIX_DB_GIT_DIRTY"),
        env!("PHOENIX_DB_PACKAGE_VERSION"),
    )
}

fn build_identity_from(
    sha: &str,
    status: &str,
    package_version: &str,
) -> DormantGitRepositoryBuildIdentity {
    let package_version = package_version.to_string();
    if sha.len() != 40
        || !sha.chars().all(|character| character.is_ascii_hexdigit())
        || status == "unknown"
    {
        DormantGitRepositoryBuildIdentity::Unavailable { package_version }
    } else if status == "dirty" {
        DormantGitRepositoryBuildIdentity::Dirty {
            sha: sha.to_string(),
            package_version,
        }
    } else if status == "clean" {
        DormantGitRepositoryBuildIdentity::ExactClean {
            sha: sha.to_string(),
            package_version,
        }
    } else {
        DormantGitRepositoryBuildIdentity::Unavailable { package_version }
    }
}

fn compiled_schema_summary(
    applied_ledger: Vec<(i64, String)>,
    inspected_r1_ddl: BTreeMap<String, String>,
) -> DormantGitRepositoryReadinessSchemaSummary {
    DormantGitRepositoryReadinessSchemaSummary {
        compiled_migration_digest: crate::migrations::compiled_migration_digest(),
        compiled_migration_count: crate::migrations::compiled_migration_ledger().len(),
        applied_ledger,
        inspected_r1_ddl,
    }
}

struct DormantGitRepositoryReadinessInspection {
    diagnostics: DormantGitRepositoryReadinessDiagnostics,
    applied_ledger: Vec<(i64, String)>,
    inspected_r1_ddl: BTreeMap<String, String>,
}

#[allow(
    clippy::too_many_lines,
    reason = "one transaction deliberately owns every readiness fact"
)]
async fn validate_dormant_git_repository_readiness_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<DormantGitRepositoryReadinessInspection> {
    let mut inspection = migration_and_schema_diagnostics(tx).await?;
    let diagnostics = &mut inspection.diagnostics;
    match compiled_build_identity() {
        DormantGitRepositoryBuildIdentity::ExactClean { .. } => {}
        DormantGitRepositoryBuildIdentity::Dirty { .. } => diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::CandidateBuildDirty,
            "compiled candidate source is dirty",
        ),
        DormantGitRepositoryBuildIdentity::Unavailable { .. } => diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::CandidateBuildUnavailable,
            "compiled candidate source identity is unavailable",
        ),
    }
    if diagnostics
        .categories
        .contains_key(&DormantGitRepositoryReadinessDiagnosticCategory::Schema)
    {
        return Ok(inspection);
    }
    let project_ids = load_expected_repositories(&mut **tx).await?;
    let project_attachments = load_expected_scope_attachments(&mut **tx).await?;
    let actual_repository_ids = load_actual_repository_ids(&mut **tx).await?;
    let actual_attachments = load_actual_attachments(&mut **tx).await?;
    let actual_locator_observations = load_actual_locator_observations(&mut **tx).await?;
    let actual_default_branch_observations =
        load_actual_default_branch_observations(&mut **tx).await?;
    let (project_attachments, conflicting_scopes) =
        resolve_expected_scope_attachments_for_validation(project_attachments);
    let (expected_repository_ids, expected_attachments) =
        seed_repository_snapshot(project_ids, project_attachments)?;

    for repository_id in expected_repository_ids.difference(&actual_repository_ids) {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::MissingRepository,
            format!(
                "missing project-seeded repository {}",
                repository_id.as_str()
            ),
        );
    }
    for repository_id in actual_repository_ids.difference(&expected_repository_ids) {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedRepository,
            format!("unexpected dormant repository {}", repository_id.as_str()),
        );
    }
    for (scope, repository_ids) in &conflicting_scopes {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::ConflictingScopeAttachment,
            format!(
                "work scope {} maps to conflicting projects {}",
                scope.as_str(),
                repository_ids
                    .iter()
                    .map(ProjectSeedId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    for (scope, expected) in &expected_attachments {
        match actual_attachments.get(scope) {
            None => diagnostics.push(
                DormantGitRepositoryReadinessDiagnosticCategory::MissingScopeAttachment,
                format!(
                    "missing attachment {} -> {}",
                    scope.as_str(),
                    expected.as_str()
                ),
            ),
            Some(actual) if actual == expected => {}
            Some(actual) => diagnostics.push(
                DormantGitRepositoryReadinessDiagnosticCategory::MismatchedScopeAttachment,
                format!(
                    "mismatched attachment {} expected {} got {}",
                    scope.as_str(),
                    expected.as_str(),
                    actual.as_str()
                ),
            ),
        }
    }
    for (scope, actual) in &actual_attachments {
        if !expected_attachments.contains_key(scope)
            && !conflicting_scopes
                .iter()
                .any(|(candidate, _)| candidate == scope)
        {
            diagnostics.push(
                DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedScopeAttachment,
                format!(
                    "unexpected attachment {} -> {}",
                    scope.as_str(),
                    actual.as_str()
                ),
            );
        }
    }
    if actual_locator_observations.is_empty() {
        diagnostics.record_valid_absence(DormantGitRepositoryValidAbsence::LocatorObservationRows);
    }
    for observation in actual_locator_observations {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedLocatorObservation,
            format!(
                "unexpected locator observation {}:{}",
                observation.repository_id.as_str(),
                observation.locator_kind
            ),
        );
    }
    if actual_default_branch_observations.is_empty() {
        diagnostics
            .record_valid_absence(DormantGitRepositoryValidAbsence::DefaultBranchObservationRows);
    }
    for observation in actual_default_branch_observations {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedDefaultBranchObservation,
            format!(
                "unexpected default-branch observation {}",
                observation.repository_id.as_str()
            ),
        );
    }
    Ok(inspection)
}

async fn migration_and_schema_diagnostics(
    connection: &mut sqlx::SqliteConnection,
) -> DbResult<DormantGitRepositoryReadinessInspection> {
    let mut diagnostics = DormantGitRepositoryReadinessDiagnostics::default();
    let applied: Vec<(i64, String)> =
        sqlx::query_as("SELECT version, name FROM _migrations ORDER BY version ASC")
            .fetch_all(&mut *connection)
            .await?;
    let expected = crate::migrations::compiled_migration_ledger();
    if applied
        .iter()
        .map(|(version, name)| (*version, name.as_str()))
        .collect::<Vec<_>>()
        != expected
    {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::MigrationLedger,
            "ordered applied migration ledger does not match compiled ledger",
        );
    }
    let expected_ddl = crate::migrations::r1_expected_table_definitions();
    let mut inspected_r1_ddl = BTreeMap::new();
    for (table, expected_definition) in expected_ddl {
        let actual: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_optional(&mut *connection)
                .await?;
        match actual.map(|sql| crate::migrations::normalize_sql(&sql)) {
            Some(actual_definition) if actual_definition == expected_definition => {
                inspected_r1_ddl.insert(table.to_string(), actual_definition);
            }
            Some(_) => diagnostics.push(
                DormantGitRepositoryReadinessDiagnosticCategory::Schema,
                format!("R1 table {table} DDL differs from migration 65"),
            ),
            None => diagnostics.push(
                DormantGitRepositoryReadinessDiagnosticCategory::Schema,
                format!("missing R1 table {table}"),
            ),
        }
    }
    let foreign_key_violations: Vec<(String, Option<i64>, String, i64)> = sqlx::query_as(
        "SELECT \"table\", rowid, parent, fkid
         FROM pragma_foreign_key_check
         ORDER BY \"table\", rowid, parent, fkid",
    )
    .fetch_all(&mut *connection)
    .await?;
    for (table, rowid, parent, fkid) in foreign_key_violations {
        let row = rowid.map_or_else(
            || "row id unavailable (WITHOUT ROWID)".to_string(),
            |rowid| format!("row {rowid}"),
        );
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::ForeignKey,
            format!("{table} {row} violates {parent} foreign key {fkid}"),
        );
    }
    Ok(DormantGitRepositoryReadinessInspection {
        diagnostics,
        applied_ledger: applied,
        inspected_r1_ddl,
    })
}

fn readiness_storage_kind_for_path(path: &str) -> DormantGitRepositoryReadinessStorageKind {
    if path.is_empty() {
        DormantGitRepositoryReadinessStorageKind::InMemory
    } else {
        DormantGitRepositoryReadinessStorageKind::FileBacked
    }
}

impl Database {
    fn claim_dormant_git_repository_readiness_receipt(
        &self,
        receipt: DormantGitRepositoryCatchupReceipt,
    ) -> DbResult<DormantGitRepositoryReadinessReceiptClaim> {
        let database_target = self.dormant_git_repository_target_binding();
        if !database_target.points_to(&receipt.target) {
            return Err(DbError::DormantGitRepositoryReadinessReceiptTargetMismatch);
        }
        let state = self.dormant_git_repository_catchup_authority_state.clone();
        let mut lifecycle = state.lifecycle.lock().expect("lifecycle mutex poisoned");
        if lifecycle.catchup_in_progress.is_some() {
            return Err(DbError::DormantGitRepositoryReadinessCatchupInProgress);
        }
        let is_latest_completed = lifecycle
            .latest_completed_marker
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &receipt.marker));
        let is_consumed = lifecycle
            .consumed_readiness_marker
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &receipt.marker));
        if !is_latest_completed || lifecycle.readiness_claim.is_some() || is_consumed {
            return Err(DbError::DormantGitRepositoryReadinessReceiptOperationMismatch);
        }
        lifecycle.readiness_claim = Some(receipt.marker.clone());
        drop(lifecycle);
        Ok(DormantGitRepositoryReadinessReceiptClaim {
            state,
            receipt,
            finalized: false,
        })
    }

    #[cfg(test)]
    fn install_dormant_git_repository_catchup_marker(
        &self,
        marker: Arc<LegacyWriterExclusionMarker>,
    ) {
        self.dormant_git_repository_catchup_authority_state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned")
            .current_marker = Some(marker);
    }

    fn record_completed_dormant_git_repository_catchup(
        &self,
        marker: &Arc<LegacyWriterExclusionMarker>,
    ) {
        let mut lifecycle = self
            .dormant_git_repository_catchup_authority_state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned");
        lifecycle.latest_completed_marker = Some(marker.clone());
        lifecycle.consumed_readiness_marker = None;
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

#[allow(
    dead_code,
    reason = "task 59004 consumes the existing dormant catch-up seam"
)]
#[derive(Debug)]
pub(crate) struct DormantGitRepositoryCatchupOutcome {
    pub stats: DormantGitRepositoryCatchupStats,
    pub receipt: DormantGitRepositoryCatchupReceipt,
}

#[allow(
    dead_code,
    reason = "task 59004 consumes the existing dormant catch-up seam"
)]
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
    project_id: ProjectSeedId,
}

#[allow(
    dead_code,
    reason = "task 59004 consumes the existing dormant catch-up seam"
)]
pub(crate) async fn catch_up_dormant_git_repositories(
    db: &Database,
    permit: DormantGitRepositoryCatchupPermit,
) -> DbResult<DormantGitRepositoryCatchupOutcome> {
    let database_target = db.dormant_git_repository_target_binding();
    if !database_target.points_to(&permit.target) {
        return Err(DbError::DormantGitRepositoryCatchupPermitTargetMismatch);
    }

    let catchup_guard = {
        let mut lifecycle = db
            .dormant_git_repository_catchup_authority_state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned");
        if lifecycle.readiness_claim.is_some() {
            return Err(DbError::DormantGitRepositoryCatchupBlockedByReadinessClaim);
        }
        let matches_current = lifecycle
            .current_marker
            .as_ref()
            .is_some_and(|marker| Arc::ptr_eq(marker, &permit.marker));
        if !matches_current {
            return Err(DbError::DormantGitRepositoryCatchupStaleOperation);
        }
        lifecycle.current_marker = None;
        lifecycle.catchup_in_progress = Some(permit.marker.clone());
        DormantGitRepositoryCatchupInProgressGuard {
            state: db.dormant_git_repository_catchup_authority_state.clone(),
            marker: permit.marker.clone(),
        }
    };

    let receipt = DormantGitRepositoryCatchupReceipt {
        target: permit.target,
        marker: permit.marker,
    };
    let mut tx = db.pool().begin().await?;
    let result = catch_up_dormant_git_repositories_tx(&mut tx).await;
    match result {
        Ok(stats) => {
            tx.commit().await?;
            db.record_completed_dormant_git_repository_catchup(&receipt.marker);
            drop(catchup_guard);
            Ok(DormantGitRepositoryCatchupOutcome { stats, receipt })
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

#[allow(
    dead_code,
    reason = "task 59004 reaches this through the dormant catch-up seam"
)]
async fn catch_up_dormant_git_repositories_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<DormantGitRepositoryCatchupStats> {
    let project_ids = load_expected_repositories(&mut **tx).await?;
    let project_attachments = load_expected_scope_attachments(&mut **tx).await?;
    let actual_repository_ids = load_actual_repository_ids(&mut **tx).await?;
    let actual_attachments = load_actual_attachments(&mut **tx).await?;
    let project_attachments = resolve_expected_scope_attachments(project_attachments)?;
    let (expected_repository_ids, expected_attachments) =
        seed_repository_snapshot(project_ids, project_attachments)?;

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

    let repository_ids_with_attachments = load_attached_repository_ids(&mut **tx).await?;
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

struct DormantGitRepositoryLocatorObservationSample {
    repository_id: GitRepositoryId,
    locator_kind: String,
}

struct DormantGitRepositoryDefaultBranchObservationSample {
    repository_id: GitRepositoryId,
}

async fn load_expected_repositories<'e, E>(executor: E) -> DbResult<BTreeSet<ProjectSeedId>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query("SELECT id FROM projects ORDER BY id")
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(|row| parse_project_seed_id(row.get::<String, _>("id")))
        .collect()
}

async fn load_actual_repository_ids<'e, E>(executor: E) -> DbResult<BTreeSet<GitRepositoryId>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query("SELECT id FROM git_repositories ORDER BY id")
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(|row| parse_git_repository_id(row.get::<String, _>("id")))
        .collect()
}

#[allow(
    dead_code,
    reason = "task 59004 reaches this through the dormant catch-up seam"
)]
async fn load_attached_repository_ids<'e, E>(executor: E) -> DbResult<BTreeSet<GitRepositoryId>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "SELECT DISTINCT repository_id FROM work_scope_git_repositories ORDER BY repository_id",
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| parse_git_repository_id(row.get::<String, _>("repository_id")))
    .collect()
}

async fn load_expected_scope_attachments<'e, E>(executor: E) -> DbResult<Vec<ExpectedAttachment>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "SELECT attachment.work_scope_id, c.project_id
         FROM conversation_work_scope_attachments attachment
         JOIN conversations c ON c.id = attachment.conversation_id
         ORDER BY attachment.work_scope_id, c.project_id, attachment.conversation_id",
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .filter_map(|row| {
        row.get::<Option<String>, _>("project_id")
            .map(|project_id| {
                Ok(ExpectedAttachment {
                    work_scope_id: parse_work_scope_id(row.get::<String, _>("work_scope_id"))?,
                    project_id: parse_project_seed_id(project_id)?,
                })
            })
    })
    .collect()
}

#[allow(
    dead_code,
    reason = "task 59004 reaches this through the dormant catch-up seam"
)]
fn resolve_expected_scope_attachments(
    attachments: Vec<ExpectedAttachment>,
) -> DbResult<BTreeMap<WorkScopeId, ProjectSeedId>> {
    let (expected, conflicts) = resolve_expected_scope_attachments_for_validation(attachments);
    if let Some((work_scope_id, project_ids)) = conflicts.into_iter().next() {
        return Err(DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            project_ids: [project_ids[0].clone(), project_ids[1].clone()],
        });
    }
    Ok(expected)
}

type ExpectedScopeAttachments = BTreeMap<WorkScopeId, ProjectSeedId>;
type ConflictingScopeAttachments = Vec<(WorkScopeId, Vec<ProjectSeedId>)>;

fn resolve_expected_scope_attachments_for_validation(
    attachments: Vec<ExpectedAttachment>,
) -> (ExpectedScopeAttachments, ConflictingScopeAttachments) {
    let mut grouped: BTreeMap<WorkScopeId, Vec<ProjectSeedId>> = BTreeMap::new();
    for attachment in attachments {
        grouped
            .entry(attachment.work_scope_id)
            .or_default()
            .push(attachment.project_id);
    }
    let mut expected = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (work_scope_id, mut project_ids) in grouped {
        project_ids.sort();
        project_ids.dedup();
        if project_ids.len() > 1 {
            conflicts.push((work_scope_id, project_ids));
        } else if let Some(project_id) = project_ids.into_iter().next() {
            expected.insert(work_scope_id, project_id);
        }
    }
    (expected, conflicts)
}

async fn load_actual_attachments<'e, E>(
    executor: E,
) -> DbResult<BTreeMap<WorkScopeId, GitRepositoryId>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "SELECT work_scope_id, repository_id FROM work_scope_git_repositories ORDER BY work_scope_id",
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            parse_work_scope_id(row.get::<String, _>("work_scope_id"))?,
            parse_git_repository_id(row.get::<String, _>("repository_id"))?,
        ))
    })
    .collect()
}

async fn load_actual_locator_observations<'e, E>(
    executor: E,
) -> DbResult<Vec<DormantGitRepositoryLocatorObservationSample>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "SELECT repository_id, locator_kind
         FROM git_repository_locator_observations
         ORDER BY repository_id, locator_kind",
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok(DormantGitRepositoryLocatorObservationSample {
            repository_id: parse_git_repository_id(row.get::<String, _>("repository_id"))?,
            locator_kind: row.get("locator_kind"),
        })
    })
    .collect()
}

async fn load_actual_default_branch_observations<'e, E>(
    executor: E,
) -> DbResult<Vec<DormantGitRepositoryDefaultBranchObservationSample>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "SELECT repository_id FROM git_repository_default_branch_observations ORDER BY repository_id",
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok(DormantGitRepositoryDefaultBranchObservationSample {
            repository_id: parse_git_repository_id(row.get::<String, _>("repository_id"))?,
        })
    })
    .collect()
}

#[allow(
    dead_code,
    reason = "task 59004 reaches this through the dormant catch-up seam"
)]
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

fn seed_repository_snapshot(
    project_ids: BTreeSet<ProjectSeedId>,
    attachments: BTreeMap<WorkScopeId, ProjectSeedId>,
) -> DbResult<(
    BTreeSet<GitRepositoryId>,
    BTreeMap<WorkScopeId, GitRepositoryId>,
)> {
    let repository_ids = project_ids
        .into_iter()
        .map(|project_id| GitRepositoryId::parse(project_id.as_str()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let attachments = attachments
        .into_iter()
        .map(|(work_scope_id, project_id)| {
            GitRepositoryId::parse(project_id.as_str())
                .map(|repository_id| (work_scope_id, repository_id))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    Ok((repository_ids, attachments))
}

fn parse_project_seed_id(value: String) -> DbResult<ProjectSeedId> {
    ProjectSeedId::parse(value).map_err(|error| DbError::Serialization(error.to_string()))
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
        insert_project(&db, "repo-a").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-a".to_string()]);
    }

    #[tokio::test]
    async fn catchup_inserts_missing_git_repository_on_file_backed_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catchup.db");
        let db = Database::open_project_authority(path.to_str().unwrap())
            .await
            .unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        insert_project(&db, "repo-file").await;

        let stats = run_catchup(&db).await.unwrap();

        assert_eq!(stats.inserted_git_repositories, 1);
        assert_eq!(repository_ids(&db).await, vec!["repo-file".to_string()]);
    }

    #[tokio::test]
    async fn catchup_replaces_superseded_attachment() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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

        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            project_ids: conflicting_project_ids,
        } = err
        else {
            panic!("expected work-scope project conflict");
        };
        assert_eq!(work_scope_id, scope);
        assert_eq!(
            conflicting_project_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["repo-a", "repo-b"]
        );

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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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

        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            project_ids: conflicting_project_ids,
        } = err
        else {
            panic!("expected work-scope project conflict");
        };
        assert_eq!(work_scope_id, scope);
        assert_eq!(
            conflicting_project_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![repo_b.to_string(), repo_a.to_string()]
        );
    }

    #[tokio::test]
    async fn catchup_conflict_reports_only_the_first_two_of_all_internal_conflicts() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let scope_a = insert_scope(&db, "scope-a").await;
        let scope_z = insert_scope(&db, "scope-z").await;
        for repository_id in ["  repo-a  ", "repo-m", "repo,z"] {
            insert_project(&db, repository_id).await;
        }
        for (id, scope, repository_id) in [
            ("a-1", &scope_a, "repo,z"),
            ("a-2", &scope_a, "  repo-a  "),
            ("a-3", &scope_a, "repo-m"),
            ("z-1", &scope_z, "repo,z"),
            ("z-2", &scope_z, "repo-m"),
        ] {
            insert_conversation_with_scope_and_project(&db, id, scope, Some(repository_id)).await;
        }

        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            project_ids,
        } = run_catchup(&db).await.unwrap_err()
        else {
            panic!("expected work-scope project conflict");
        };
        assert_eq!(work_scope_id, scope_a);
        assert_eq!(
            project_ids.map(|id| id.as_str().to_string()),
            ["  repo-a  ".to_string(), "repo,z".to_string()]
        );
    }

    #[test]
    fn attachment_conflict_collection_retains_all_internal_ids_before_the_typed_error_selects_two()
    {
        let scope = WorkScopeId::parse("scope-full-conflict").unwrap();
        let attachments = ["repo,z", "  repo-a  ", "repo-m"]
            .into_iter()
            .map(|project_id| ExpectedAttachment {
                work_scope_id: scope.clone(),
                project_id: ProjectSeedId::parse(project_id).unwrap(),
            })
            .collect();
        let (_, conflicts) = resolve_expected_scope_attachments_for_validation(attachments);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, scope);
        assert_eq!(
            conflicts[0]
                .1
                .iter()
                .map(ProjectSeedId::as_str)
                .collect::<Vec<_>>(),
            vec!["  repo-a  ", "repo,z", "repo-m"]
        );
    }

    #[tokio::test]
    async fn catchup_deletes_all_observations_even_for_retained_attached_repo() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let other_db = Database::open_in_memory_project_authority().await.unwrap();
        insert_project(&db, "repo-a").await;
        let permit = DormantGitRepositoryCatchupPermit::test_only_mint(
            &other_db,
            TestDormantGitRepositoryExclusionProof::new(),
        );

        let error = db
            .catch_up_dormant_git_repositories(permit)
            .await
            .unwrap_err();

        assert_target_mismatch(&error);
        assert!(repository_ids(&db).await.is_empty());
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn catchup_rejects_independently_reopened_file_database_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reopened-catchup.db");
        let db = Database::open_project_authority(path.to_str().unwrap())
            .await
            .unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        let reopened = Database::open_project_authority(path.to_str().unwrap())
            .await
            .unwrap();
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

        assert_target_mismatch(&error);
        assert!(repository_ids(&db).await.is_empty());
        assert_eq!(count_locator_rows(&db).await, 0);
        assert_eq!(count_default_branch_rows(&db).await, 0);
    }

    #[tokio::test]
    async fn newer_mint_supersedes_older_and_stale_rejection_leaves_newer_usable() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        assert_stale_operation(&stale);
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        assert_stale_operation(&stale);
    }

    #[tokio::test]
    async fn begin_failure_spends_operation_without_restoring_marker() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
        assert_stale_operation(&stale);
    }

    #[tokio::test]
    async fn permit_consumption_prevents_reuse_by_move() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
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
                repository_id, locator_kind, status, path, observed_at_unix_micros
             ) VALUES (?1, 'common_dir', 'present', '/tmp/common', ?2)",
        )
        .bind(repository_id)
        .bind(Utc::now().timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn insert_default_branch_observation(db: &Database, repository_id: &str) {
        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES (?1, 1, 'resolved', 'main', 'user_selected', ?2)",
        )
        .bind(repository_id)
        .bind(Utc::now().timestamp_micros())
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

    #[derive(Debug, PartialEq, Eq)]
    struct ReadinessRowsSnapshot {
        projects: Vec<(String, String, String)>,
        source_attachments: Vec<(String, String, Option<String>)>,
        repositories: Vec<String>,
        attachments: Vec<(String, String)>,
        locator_observations: Vec<(String, String, String, String, i64)>,
        default_branch_observations: Vec<(String, i64, String, Option<String>, String, i64)>,
    }

    async fn readiness_rows_snapshot(db: &Database) -> ReadinessRowsSnapshot {
        ReadinessRowsSnapshot {
            projects: sqlx::query_as(
                "SELECT id, canonical_path, main_ref FROM projects ORDER BY id",
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
            source_attachments: sqlx::query_as(
                "SELECT attachment.work_scope_id, attachment.conversation_id, c.project_id
                 FROM conversation_work_scope_attachments attachment
                 JOIN conversations c ON c.id = attachment.conversation_id
                 ORDER BY attachment.work_scope_id, attachment.conversation_id",
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
            repositories: repository_ids(db).await,
            attachments: attachment_rows(db).await,
            locator_observations: sqlx::query_as(
                "SELECT repository_id, locator_kind, status, path, observed_at_unix_micros
                 FROM git_repository_locator_observations
                 ORDER BY repository_id, locator_kind",
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
            default_branch_observations: sqlx::query_as(
                "SELECT repository_id, generation, status, branch, provenance, observed_at_unix_micros
                 FROM git_repository_default_branch_observations
                 ORDER BY repository_id, generation",
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
        }
    }

    async fn query_only_write_rejection_fixture(
        db: &Database,
    ) -> DormantGitRepositoryReadinessInspection {
        let mut guard = QueryOnlyConnectionGuard::acquire(db).await.unwrap();
        let original_query_only = read_query_only_flag(guard.connection()).await.unwrap();
        guard.arm();
        set_query_only_flag(guard.connection(), true).await.unwrap();
        let mut tx = begin_query_only_transaction(guard.connection())
            .await
            .unwrap();

        let write =
            sqlx::query("INSERT INTO git_repositories (id) VALUES ('query-only-write-probe')")
                .execute(&mut *tx)
                .await;

        let error = write.expect_err("query_only must reject writes inside readiness snapshot");
        assert!(
            error.to_string().contains("readonly"),
            "unexpected query_only write error: {error}"
        );
        let inspection = validate_dormant_git_repository_readiness_tx(&mut tx)
            .await
            .expect("readiness inspection remains usable after the rejected write probe");
        tx.rollback().await.unwrap();
        set_query_only_flag(guard.connection(), original_query_only)
            .await
            .unwrap();
        guard.disarm();
        inspection
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

    async fn validate_readiness(
        db: &Database,
        receipt: DormantGitRepositoryCatchupReceipt,
    ) -> DbResult<DormantGitRepositoryCanonicalReadinessEvidence> {
        validate_dormant_git_repository_readiness(db, receipt).await
    }

    fn assert_readiness_target_mismatch(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryReadinessReceiptTargetMismatch
        ));
    }

    fn assert_readiness_operation_mismatch(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryReadinessReceiptOperationMismatch
        ));
    }

    fn assert_target_mismatch(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryCatchupPermitTargetMismatch
        ));
    }

    fn assert_stale_operation(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryCatchupStaleOperation
        ));
    }

    fn assert_catchup_blocked_by_readiness_claim(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryCatchupBlockedByReadinessClaim
        ));
    }

    fn assert_readiness_catchup_in_progress(error: &DbError) {
        assert!(matches!(
            error,
            DbError::DormantGitRepositoryReadinessCatchupInProgress
        ));
    }

    #[tokio::test]
    async fn readiness_derives_internal_build_identity_schema_and_fresh_root() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let first = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let first_evidence = validate_readiness(&db, first.receipt).await.unwrap();
        let second = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let second_evidence = validate_readiness(&db, second.receipt).await.unwrap();

        assert!(first_evidence
            .diagnostics()
            .iter()
            .all(|sample| !sample.starts_with("Schema:")));
        assert_eq!(
            first_evidence.summary().storage_kind(),
            DormantGitRepositoryReadinessStorageKind::InMemory
        );
        match &first_evidence.build {
            DormantGitRepositoryBuildIdentity::ExactClean { sha, .. }
            | DormantGitRepositoryBuildIdentity::Dirty { sha, .. } => {
                assert_eq!(sha.len(), 40);
                assert!(sha.chars().all(|character| character.is_ascii_hexdigit()));
            }
            DormantGitRepositoryBuildIdentity::Unavailable { .. } => {}
        }
        assert!(second_evidence.has_fresh_root_from(&first_evidence));
        assert_ne!(
            second_evidence.run_root().run_id,
            first_evidence.run_root().run_id,
            "identical readiness observations receive distinct actual run identities"
        );
    }

    #[test]
    fn readiness_build_identity_fails_closed_for_dirty_unknown_and_invalid_metadata() {
        assert!(matches!(
            build_identity_from("0123456789abcdef0123456789abcdef01234567", "clean", "1.2.3"),
            DormantGitRepositoryBuildIdentity::ExactClean { .. }
        ));
        assert!(matches!(
            build_identity_from("0123456789abcdef0123456789abcdef01234567", "dirty", "1.2.3"),
            DormantGitRepositoryBuildIdentity::Dirty { .. }
        ));
        assert!(matches!(
            build_identity_from("unknown", "unknown", "1.2.3"),
            DormantGitRepositoryBuildIdentity::Unavailable { .. }
        ));
        assert!(matches!(
            build_identity_from("not-a-sha", "clean", "1.2.3"),
            DormantGitRepositoryBuildIdentity::Unavailable { .. }
        ));
    }

    #[tokio::test]
    async fn readiness_receipt_rejects_target_mismatch_and_reuse() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let other = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let reused_receipt = DormantGitRepositoryCatchupReceipt {
            target: outcome.receipt.target.clone(),
            marker: outcome.receipt.marker.clone(),
        };
        let wrong_target_receipt = DormantGitRepositoryCatchupReceipt {
            target: other.dormant_git_repository_target_binding().state,
            marker: outcome.receipt.marker.clone(),
        };

        validate_readiness(&db, outcome.receipt).await.unwrap();
        assert_readiness_operation_mismatch(
            &validate_readiness(&db, reused_receipt).await.unwrap_err(),
        );
        assert_readiness_target_mismatch(
            &validate_readiness(&db, wrong_target_receipt)
                .await
                .unwrap_err(),
        );
    }

    #[tokio::test]
    async fn readiness_receipt_claim_rejects_concurrency_but_releases_without_finalization() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let concurrent_receipt = DormantGitRepositoryCatchupReceipt {
            target: outcome.receipt.target.clone(),
            marker: outcome.receipt.marker.clone(),
        };
        let retry_receipt = DormantGitRepositoryCatchupReceipt {
            target: outcome.receipt.target.clone(),
            marker: outcome.receipt.marker.clone(),
        };

        let claim = db
            .claim_dormant_git_repository_readiness_receipt(outcome.receipt)
            .unwrap();
        let Err(concurrent_error) =
            db.claim_dormant_git_repository_readiness_receipt(concurrent_receipt)
        else {
            panic!("concurrent claim unexpectedly succeeded");
        };
        assert_readiness_operation_mismatch(&concurrent_error);
        drop(claim);

        validate_readiness(&db, retry_receipt).await.unwrap();
    }

    #[tokio::test]
    async fn only_the_latest_successful_catchup_receipt_may_be_claimed() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let first = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let stale_first_receipt = DormantGitRepositoryCatchupReceipt {
            target: first.receipt.target.clone(),
            marker: first.receipt.marker.clone(),
        };
        let second = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();

        assert_readiness_operation_mismatch(
            &validate_readiness(&db, stale_first_receipt)
                .await
                .unwrap_err(),
        );
        validate_readiness(&db, second.receipt).await.unwrap();
    }

    #[tokio::test]
    async fn active_readiness_claim_blocks_catchup_without_spending_its_marker() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let claim = db
            .claim_dormant_git_repository_readiness_receipt(outcome.receipt)
            .unwrap();
        let proof_b = TestDormantGitRepositoryExclusionProof::new();
        let blocked_permit =
            DormantGitRepositoryCatchupPermit::test_only_mint(&db, proof_b.clone());

        assert_catchup_blocked_by_readiness_claim(
            &db.catch_up_dormant_git_repositories(blocked_permit)
                .await
                .unwrap_err(),
        );
        drop(claim);

        db.catch_up_dormant_git_repositories(DormantGitRepositoryCatchupPermit::test_only_mint(
            &db, proof_b,
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn readiness_claim_and_catchup_start_share_one_lifecycle_coordination_state() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let receipt = DormantGitRepositoryCatchupReceipt {
            target: outcome.receipt.target.clone(),
            marker: outcome.receipt.marker.clone(),
        };
        let state = db.dormant_git_repository_catchup_authority_state.clone();
        let in_progress = Arc::new(LegacyWriterExclusionMarker);
        let guard = DormantGitRepositoryCatchupInProgressGuard {
            state: state.clone(),
            marker: in_progress.clone(),
        };
        state
            .lifecycle
            .lock()
            .expect("lifecycle mutex poisoned")
            .catchup_in_progress = Some(in_progress);

        assert_readiness_catchup_in_progress(&validate_readiness(&db, receipt).await.unwrap_err());
        drop(guard);

        validate_readiness(&db, outcome.receipt).await.unwrap();
    }

    #[tokio::test]
    async fn readiness_validation_error_restores_connection_and_releases_receipt_for_retry() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let retry_receipt = DormantGitRepositoryCatchupReceipt {
            target: outcome.receipt.target.clone(),
            marker: outcome.receipt.marker.clone(),
        };
        sqlx::query("ALTER TABLE projects RENAME TO projects_unavailable")
            .execute(db.pool())
            .await
            .unwrap();

        assert!(matches!(
            validate_readiness(&db, outcome.receipt).await.unwrap_err(),
            DbError::Sqlx(_)
        ));
        let mut conn = db.pool().acquire().await.unwrap();
        assert!(!read_query_only_flag(&mut *conn).await.unwrap());
        sqlx::query("ALTER TABLE projects_unavailable RENAME TO projects")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        validate_readiness(&db, retry_receipt).await.unwrap();
    }

    #[tokio::test]
    async fn readiness_reports_ordered_migration_ledger_mismatch() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        sqlx::query("DELETE FROM _migrations WHERE version = 65")
            .execute(db.pool())
            .await
            .unwrap();

        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();

        assert_eq!(
            evidence.eligibility(),
            &DormantGitRepositoryR1Eligibility::Ineligible
        );
        assert!(evidence
            .diagnostic_categories()
            .contains(&DormantGitRepositoryReadinessDiagnosticCategory::MigrationLedger));
    }

    #[tokio::test]
    async fn readiness_reports_r1_schema_and_foreign_key_integrity_categories() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let schema_outcome =
            run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
                .await
                .unwrap();
        sqlx::query("DROP TABLE git_repository_default_branch_observations")
            .execute(db.pool())
            .await
            .unwrap();
        let schema = validate_readiness(&db, schema_outcome.receipt)
            .await
            .unwrap();
        assert!(schema
            .diagnostic_categories()
            .contains(&DormantGitRepositoryReadinessDiagnosticCategory::Schema));

        let db = Database::open_in_memory_project_authority().await.unwrap();
        let ddl_outcome =
            run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
                .await
                .unwrap();
        sqlx::query("DROP TABLE git_repositories")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("CREATE TABLE git_repositories (id TEXT PRIMARY KEY)")
            .execute(db.pool())
            .await
            .unwrap();
        let ddl_drift = validate_readiness(&db, ddl_outcome.receipt).await.unwrap();
        assert!(ddl_drift
            .diagnostic_categories()
            .contains(&DormantGitRepositoryReadinessDiagnosticCategory::Schema));

        let db = Database::open_in_memory_project_authority().await.unwrap();
        let fk_outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id) VALUES ('missing-scope', 'missing-repository')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(db.pool())
            .await
            .unwrap();
        let foreign_key = validate_readiness(&db, fk_outcome.receipt).await.unwrap();
        assert!(foreign_key
            .diagnostic_categories()
            .contains(&DormantGitRepositoryReadinessDiagnosticCategory::ForeignKey));
    }

    #[tokio::test]
    async fn readiness_reports_without_rowid_foreign_key_violation_with_unavailable_row_id() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let mut connection = db.pool().acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workflow_supported_codecs (workflow_id, codec_family, codec_version)
             VALUES (9223372036854775807, 'missing-workflow', 1)",
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);

        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();
        let foreign_key = evidence
            .summary()
            .diagnostic_categories()
            .iter()
            .find(|category| {
                category.category() == DormantGitRepositoryReadinessDiagnosticCategory::ForeignKey
            })
            .expect("WITHOUT ROWID violation remains typed ForeignKey evidence");
        assert_eq!(foreign_key.total_count(), 1);
        assert!(foreign_key.samples().iter().any(|sample| {
            sample.detail().contains("workflow_supported_codecs")
                && sample
                    .detail()
                    .contains("row id unavailable (WITHOUT ROWID)")
        }));
    }

    #[tokio::test]
    async fn readiness_records_explicit_valid_absence_for_unretained_observations() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();

        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();

        assert!(
            evidence.has_valid_absence(DormantGitRepositoryValidAbsence::LocatorObservationRows)
        );
        assert!(evidence
            .has_valid_absence(DormantGitRepositoryValidAbsence::DefaultBranchObservationRows));
        assert_eq!(
            evidence.summary().valid_absences(),
            [
                DormantGitRepositoryValidAbsence::LocatorObservationRows,
                DormantGitRepositoryValidAbsence::DefaultBranchObservationRows,
            ]
        );
    }

    #[tokio::test]
    async fn readiness_query_only_rejects_writes_and_preserves_every_validated_row() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        insert_project(&db, "repo-query-only").await;
        let scope = insert_scope(&db, "scope-query-only").await;
        insert_conversation_with_scope_and_project(
            &db,
            "conv-query-only",
            &scope,
            Some("repo-query-only"),
        )
        .await;
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        let before = readiness_rows_snapshot(&db).await;

        let inspection = query_only_write_rejection_fixture(&db).await;
        assert_eq!(
            inspection.diagnostics.typed_valid_absences(),
            [
                DormantGitRepositoryValidAbsence::LocatorObservationRows,
                DormantGitRepositoryValidAbsence::DefaultBranchObservationRows,
            ]
        );
        assert!(inspection
            .diagnostics
            .categories
            .keys()
            .all(|category| matches!(
                category,
                DormantGitRepositoryReadinessDiagnosticCategory::CandidateBuildDirty
                    | DormantGitRepositoryReadinessDiagnosticCategory::CandidateBuildUnavailable
            )));
        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();

        assert_eq!(
            evidence.summary().valid_absences(),
            inspection.diagnostics.typed_valid_absences()
        );
        assert_eq!(readiness_rows_snapshot(&db).await, before);
    }

    #[tokio::test]
    async fn readiness_query_only_restores_connection_and_keeps_in_memory_database_alive() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        insert_project(&db, "repo-a").await;
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();

        validate_readiness(&db, outcome.receipt).await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();
        assert!(!read_query_only_flag(&mut *conn).await.unwrap());
        sqlx::query("INSERT INTO git_repositories (id) VALUES ('repo-after')")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        assert_eq!(repository_ids(&db).await, vec!["repo-a", "repo-after"]);
    }

    #[tokio::test]
    async fn readiness_diagnostics_are_deterministic_and_bounded() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        for id in 0..12 {
            insert_project(&db, &format!("repo-{id:02}")).await;
        }
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        for id in 0..12 {
            sqlx::query("DELETE FROM git_repositories WHERE id = ?1")
                .bind(format!("repo-{id:02}"))
                .execute(db.pool())
                .await
                .unwrap();
        }
        insert_git_repository(&db, "repo-observed").await;
        insert_locator_observation(&db, "repo-observed").await;

        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();
        assert_eq!(
            evidence.diagnostic_count(
                DormantGitRepositoryReadinessDiagnosticCategory::MissingRepository
            ),
            12
        );
        assert_eq!(
            evidence.diagnostic_count(
                DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedLocatorObservation
            ),
            1
        );
        assert!(evidence.diagnostic_categories().contains(
            &DormantGitRepositoryReadinessDiagnosticCategory::UnexpectedLocatorObservation,
        ));
        let missing = evidence
            .summary()
            .diagnostic_categories()
            .iter()
            .find(|category| {
                category.category()
                    == DormantGitRepositoryReadinessDiagnosticCategory::MissingRepository
            })
            .unwrap();
        assert_eq!(missing.total_count(), 12);
        assert_eq!(missing.samples().len(), READINESS_SAMPLE_LIMIT);
        assert_eq!(
            evidence.diagnostics().first().unwrap(),
            "MissingRepository: missing project-seeded repository repo-00"
        );
        assert_eq!(
            evidence.diagnostics().get(9).unwrap(),
            "MissingRepository: missing project-seeded repository repo-09"
        );
    }

    #[test]
    fn expected_attachment_preserves_typed_project_identity_until_seed_conversion() {
        let attachment = ExpectedAttachment {
            work_scope_id: WorkScopeId::parse("scope-typed").unwrap(),
            project_id: ProjectSeedId::parse("  project,typed  ").unwrap(),
        };
        assert_eq!(attachment.work_scope_id.as_str(), "scope-typed");
        assert_eq!(attachment.project_id.as_str(), "  project,typed  ");
        assert_eq!(
            seed_repository_snapshot(
                BTreeSet::from([attachment.project_id.clone()]),
                BTreeMap::new()
            )
            .unwrap()
            .0
            .into_iter()
            .next()
            .unwrap()
            .as_str(),
            attachment.project_id.as_str()
        );
    }
}
