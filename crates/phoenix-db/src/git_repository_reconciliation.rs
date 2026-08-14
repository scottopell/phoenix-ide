use crate::{Database, DbError, DbResult};
use phoenix_core::git_repository::GitRepositoryId;
use phoenix_core::work_scope::WorkScopeId;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DormantGitRepositoryR1Eligibility {
    Eligible,
    Ineligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

impl DormantGitRepositoryCanonicalReadinessEvidence {
    fn unit4_binding(&self) -> DormantGitRepositoryUnit4RunBinding {
        DormantGitRepositoryUnit4RunBinding {
            database: self.root.database.clone(),
            operation: self.root.operation.clone(),
            run_marker: self.root.run_marker.clone(),
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

#[derive(Debug, Clone)]
struct DormantGitRepositoryUnit4RunBinding {
    database: Arc<DormantGitRepositoryCatchupAuthorityState>,
    operation: Arc<LegacyWriterExclusionMarker>,
    run_marker: Arc<DormantGitRepositoryReadinessRunMarker>,
}

impl DormantGitRepositoryUnit4RunBinding {
    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.database, &other.database)
            && Arc::ptr_eq(&self.operation, &other.operation)
            && Arc::ptr_eq(&self.run_marker, &other.run_marker)
    }
}

#[derive(Debug)]
struct DormantGitRepositorySourceCensusAttestation {
    root: DormantGitRepositoryUnit4RunBinding,
    revision: String,
    content_digest: String,
    shadow_reference_count: usize,
    project_authority_path_count: usize,
    clean: bool,
}

#[derive(Debug)]
struct DormantGitRepositoryOldBinaryCompatibilityAttestation {
    root: DormantGitRepositoryUnit4RunBinding,
    historical_sha: String,
    candidate_schema_digest: String,
    old_source_unchanged: bool,
    catchup_exact: bool,
    replay_idempotent: bool,
    fresh_readiness_root: bool,
    passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DormantGitRepositoryRollbackPosture {
    AdditiveSchemaRetainedDestructiveDownMigrationProhibited,
}

impl DormantGitRepositoryRollbackPosture {
    fn as_str(self) -> &'static str {
        "additive_schema_retained_destructive_down_migration_prohibited"
    }
}

#[derive(Debug)]
struct DormantGitRepositoryRollbackAttestation {
    root: DormantGitRepositoryUnit4RunBinding,
    posture: DormantGitRepositoryRollbackPosture,
    additive_schema_retained: bool,
    old_source_unchanged: bool,
}

/// An opaque, in-process acceptance aggregate. Its nonce is represented by an
/// Arc identity and deliberately has no cross-process stability contract.
#[derive(Debug)]
struct DormantGitRepositoryCompleteCompatibilityEvidence {
    readiness: DormantGitRepositoryCanonicalReadinessEvidence,
    census: DormantGitRepositorySourceCensusAttestation,
    compatibility: DormantGitRepositoryOldBinaryCompatibilityAttestation,
    rollback: DormantGitRepositoryRollbackAttestation,
    integrity_digest: String,
    eligible: bool,
}

impl DormantGitRepositoryCompleteCompatibilityEvidence {
    fn new(
        readiness: DormantGitRepositoryCanonicalReadinessEvidence,
        census: DormantGitRepositorySourceCensusAttestation,
        compatibility: DormantGitRepositoryOldBinaryCompatibilityAttestation,
        rollback: DormantGitRepositoryRollbackAttestation,
        target_identity: &str,
    ) -> DbResult<Self> {
        let root = readiness.unit4_binding();
        if !root.matches(&census.root)
            || !root.matches(&compatibility.root)
            || !root.matches(&rollback.root)
        {
            return Err(DbError::Serialization(
                "compatibility evidence members were assembled from different runs".to_string(),
            ));
        }
        let build = compiled_build_identity();
        let DormantGitRepositoryBuildIdentity::ExactClean { sha, .. } = &build else {
            return Err(DbError::Serialization(
                "candidate build must be an exact clean commit; commit the candidate before running the compatibility finalizer"
                    .to_string(),
            ));
        };
        let eligibility = readiness.summary.eligibility == DormantGitRepositoryR1Eligibility::Eligible
            && census.clean
            && compatibility.passed
            && compatibility.old_source_unchanged
            && compatibility.catchup_exact
            && compatibility.replay_idempotent
            && compatibility.fresh_readiness_root
            && rollback.additive_schema_retained
            && rollback.old_source_unchanged
            && rollback.posture
                == DormantGitRepositoryRollbackPosture::AdditiveSchemaRetainedDestructiveDownMigrationProhibited;
        let integrity_digest = length_framed_digest([
            ("candidate_build", sha.as_bytes()),
            (
                "candidate_schema",
                readiness.schema.compiled_migration_digest.as_bytes(),
            ),
            ("target_identity", target_identity.as_bytes()),
            ("census_revision", census.revision.as_bytes()),
            ("census_digest", census.content_digest.as_bytes()),
            ("historical_build", compatibility.historical_sha.as_bytes()),
            (
                "compatibility_schema",
                compatibility.candidate_schema_digest.as_bytes(),
            ),
            ("rollback_posture", rollback.posture.as_str().as_bytes()),
            ("eligible", if eligibility { b"true" } else { b"false" }),
        ]);
        Ok(Self {
            readiness,
            census,
            compatibility,
            rollback,
            integrity_digest,
            eligible: eligibility,
        })
    }

    #[cfg(test)]
    fn artifact_fields(&self) -> (&str, &str, &str, &str, usize, usize, bool, &str) {
        let DormantGitRepositoryBuildIdentity::ExactClean { sha, .. } = &self.readiness.build
        else {
            unreachable!("complete evidence requires an exact clean candidate build");
        };
        (
            sha,
            &self.readiness.schema.compiled_migration_digest,
            &self.compatibility.historical_sha,
            &self.census.revision,
            self.census.shadow_reference_count,
            self.census.project_authority_path_count,
            self.eligible,
            &self.integrity_digest,
        )
    }

    #[cfg(test)]
    fn rollback_posture(&self) -> &'static str {
        self.rollback.posture.as_str()
    }

    #[cfg(test)]
    fn census_digest(&self) -> &str {
        &self.census.content_digest
    }
}

fn length_framed_digest<'a>(parts: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut hasher = Sha256::new();
    for (name, value) in parts {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn assert_additive_shadow_schema(path: &str) -> DbResult<()> {
    if path.is_empty() || !Path::new(path).is_file() {
        return Err(DbError::Serialization(
            "compatibility finalizer requires a file-backed database".to_string(),
        ));
    }
    Ok(())
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
            let diagnostics = inspection.diagnostics;
            let build = compiled_build_identity();
            let schema =
                compiled_schema_summary(inspection.applied_ledger, inspection.inspected_r1_ddl);
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
    let expected_repository_ids = load_expected_repositories(&mut **tx).await?;
    let expected_scope_attachments = load_expected_scope_attachments(&mut **tx).await?;
    let actual_repository_ids = load_actual_repository_ids(&mut **tx).await?;
    let actual_attachments = load_actual_attachments(&mut **tx).await?;
    let actual_locator_observations = load_actual_locator_observations(&mut **tx).await?;
    let actual_default_branch_observations =
        load_actual_default_branch_observations(&mut **tx).await?;
    let (expected_attachments, conflicting_scopes) =
        resolve_expected_scope_attachments_for_validation(expected_scope_attachments);

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
                    .map(GitRepositoryId::as_str)
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
    let foreign_key_violations: Vec<(String, i64, String, i64)> = sqlx::query_as("SELECT \"table\", rowid, parent, fkid FROM pragma_foreign_key_check ORDER BY \"table\", rowid, parent, fkid").fetch_all(&mut *connection).await?;
    for (table, rowid, parent, fkid) in foreign_key_violations {
        diagnostics.push(
            DormantGitRepositoryReadinessDiagnosticCategory::ForeignKey,
            format!("{table} row {rowid} violates {parent} foreign key {fkid}"),
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
    repository_id: GitRepositoryId,
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
    let expected_repository_ids = load_expected_repositories(&mut **tx).await?;
    let expected_scope_attachments = load_expected_scope_attachments(&mut **tx).await?;
    let actual_repository_ids = load_actual_repository_ids(&mut **tx).await?;
    let actual_attachments = load_actual_attachments(&mut **tx).await?;
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

async fn load_expected_repositories<'e, E>(executor: E) -> DbResult<BTreeSet<GitRepositoryId>>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query("SELECT id FROM projects ORDER BY id")
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(|row| parse_git_repository_id(row.get::<String, _>("id")))
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
                    repository_id: parse_git_repository_id(project_id)?,
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
) -> DbResult<BTreeMap<WorkScopeId, GitRepositoryId>> {
    let (expected, conflicts) = resolve_expected_scope_attachments_for_validation(attachments);
    if let Some((work_scope_id, repository_ids)) = conflicts.into_iter().next() {
        return Err(DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            repository_ids,
        });
    }
    Ok(expected)
}

type ExpectedScopeAttachments = BTreeMap<WorkScopeId, GitRepositoryId>;
type ConflictingScopeAttachments = Vec<(WorkScopeId, Vec<GitRepositoryId>)>;

fn resolve_expected_scope_attachments_for_validation(
    attachments: Vec<ExpectedAttachment>,
) -> (ExpectedScopeAttachments, ConflictingScopeAttachments) {
    let mut grouped: BTreeMap<WorkScopeId, Vec<GitRepositoryId>> = BTreeMap::new();
    for attachment in attachments {
        grouped
            .entry(attachment.work_scope_id)
            .or_default()
            .push(attachment.repository_id);
    }
    let mut expected = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (work_scope_id, mut repository_ids) in grouped {
        repository_ids.sort();
        repository_ids.dedup();
        if repository_ids.len() > 1 {
            conflicts.push((work_scope_id, repository_ids));
        } else if let Some(repository_id) = repository_ids.into_iter().next() {
            expected.insert(work_scope_id, repository_id);
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
    use std::env;
    use std::fs;

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

        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            repository_ids: conflicting_repository_ids,
        } = err
        else {
            panic!("expected work-scope project conflict");
        };
        assert_eq!(work_scope_id, scope);
        assert_eq!(
            conflicting_repository_ids
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

        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            repository_ids: conflicting_repository_ids,
        } = err
        else {
            panic!("expected work-scope project conflict");
        };
        assert_eq!(work_scope_id, scope);
        assert_eq!(
            conflicting_repository_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![repo_b.to_string(), repo_a.to_string()]
        );
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

        assert_target_mismatch(&error);
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

        assert_target_mismatch(&error);
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
        assert_stale_operation(&stale);
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
        assert_stale_operation(&stale);
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

    #[derive(Debug, PartialEq, Eq)]
    struct ReadinessRowsSnapshot {
        projects: Vec<(String, String, String)>,
        source_attachments: Vec<(String, String, Option<String>)>,
        repositories: Vec<String>,
        attachments: Vec<(String, String)>,
        locator_observations: Vec<(String, String, String, Option<String>, String)>,
        default_branch_observations: Vec<(String, i64, String, Option<String>, String, String)>,
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
                "SELECT repository_id, locator_kind, status, path, observed_at
                 FROM git_repository_locator_observations
                 ORDER BY repository_id, locator_kind",
            )
            .fetch_all(db.pool())
            .await
            .unwrap(),
            default_branch_observations: sqlx::query_as(
                "SELECT repository_id, generation, status, branch, provenance, observed_at
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
        let other = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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

        let db = Database::open_in_memory().await.unwrap();
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

        let db = Database::open_in_memory().await.unwrap();
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
    async fn readiness_records_explicit_valid_absence_for_unretained_observations() {
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
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
        let db = Database::open_in_memory().await.unwrap();
        insert_project(&db, "repo-a").await;
        let outcome = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();

        let evidence = validate_readiness(&db, outcome.receipt).await.unwrap();
        assert!(matches!(
            evidence.eligibility(),
            DormantGitRepositoryR1Eligibility::Eligible
                | DormantGitRepositoryR1Eligibility::Ineligible
        ));
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
        let db = Database::open_in_memory().await.unwrap();
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

    #[derive(serde::Serialize)]
    struct CompatibilityFinalizerArtifact {
        candidate_sha: String,
        candidate_schema_digest: String,
        historical_sha: String,
        census_revision: String,
        census_content_digest: String,
        shadow_reference_count: usize,
        project_authority_path_count: usize,
        rollback_posture: String,
        complete_eligibility: bool,
        integrity_digest: String,
        catchup_inserted_repositories: usize,
        catchup_inserted_attachments: usize,
    }

    fn required_env(name: &str) -> String {
        env::var(name)
            .unwrap_or_else(|_| panic!("{name} is required for the compatibility finalizer"))
    }

    async fn shadow_counts(db: &Database) -> Vec<(String, i64)> {
        vec![
            (
                "git_repositories".to_string(),
                sqlx::query_scalar("SELECT COUNT(*) FROM git_repositories")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
            ),
            (
                "work_scope_git_repositories".to_string(),
                sqlx::query_scalar("SELECT COUNT(*) FROM work_scope_git_repositories")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
            ),
            (
                "git_repository_locator_observations".to_string(),
                sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_locator_observations")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
            ),
            (
                "git_repository_default_branch_observations".to_string(),
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM git_repository_default_branch_observations",
                )
                .fetch_one(db.pool())
                .await
                .unwrap(),
            ),
        ]
    }

    async fn source_snapshot(
        db: &Database,
        canonical_path: &str,
        conversation_id: &str,
    ) -> (i64, Option<String>, Option<String>, Option<String>) {
        let project_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projects WHERE canonical_path = ?1")
                .bind(canonical_path)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let project_id =
            sqlx::query_scalar::<_, String>("SELECT project_id FROM conversations WHERE id = ?1")
                .bind(conversation_id)
                .fetch_optional(db.pool())
                .await
                .unwrap();
        let state_kind =
            sqlx::query_scalar::<_, String>("SELECT state_kind FROM conversations WHERE id = ?1")
                .bind(conversation_id)
                .fetch_optional(db.pool())
                .await
                .unwrap();
        let job_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM conversation_creation_jobs WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(db.pool())
        .await
        .unwrap();
        (project_count, project_id, state_kind, job_status)
    }

    #[tokio::test]
    #[ignore = "historical binary acceptance runner supplies a migrated file database"]
    async fn finalizes_historical_r1_compatibility_handoff() {
        let db_path = required_env("PHOENIX_R1_COMPAT_DB_PATH");
        let canonical_path = required_env("PHOENIX_R1_COMPAT_CANONICAL_PATH");
        let project_id = required_env("PHOENIX_R1_COMPAT_PROJECT_ID");
        let conversation_id = required_env("PHOENIX_R1_COMPAT_CONVERSATION_ID");
        let artifact_path = required_env("PHOENIX_R1_COMPAT_FINALIZER_ARTIFACT");
        let historical_sha = required_env("PHOENIX_R1_COMPAT_HISTORICAL_SHA");
        let census_revision = required_env("PHOENIX_R1_COMPAT_CENSUS_REVISION");
        let census_content_digest = required_env("PHOENIX_R1_COMPAT_CENSUS_DIGEST");
        let shadow_reference_count = required_env("PHOENIX_R1_COMPAT_SHADOW_REFERENCE_COUNT")
            .parse::<usize>()
            .unwrap();
        let project_authority_path_count =
            required_env("PHOENIX_R1_COMPAT_PROJECT_AUTHORITY_PATH_COUNT")
                .parse::<usize>()
                .unwrap();
        assert_eq!(
            historical_sha, "799ea4d63c3d451f3f47859fa21df46fe3072923",
            "the compatibility finalizer must be tied to the frozen historical binary"
        );
        assert_additive_shadow_schema(&db_path).unwrap();
        let db = Database::open(&db_path).await.unwrap();
        let before_source = source_snapshot(&db, &canonical_path, &conversation_id).await;
        assert_eq!(
            before_source.0, 1,
            "historical process created one canonical project"
        );
        assert_eq!(before_source.1.as_deref(), Some(project_id.as_str()));
        assert_eq!(before_source.2.as_deref(), Some("idle"));
        assert_eq!(before_source.3.as_deref(), Some("ready"));
        let before_shadow = shadow_counts(&db).await;
        assert!(
            before_shadow.iter().all(|(_, count)| *count == 0),
            "the historical process must not write R1 shadow tables: {before_shadow:?}"
        );

        let first = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        assert_eq!(first.stats.inserted_git_repositories, 1);
        assert_eq!(first.stats.inserted_work_scope_attachments, 1);
        assert_eq!(first.stats.deleted_git_repositories, 0);
        assert_eq!(first.stats.deleted_work_scope_attachments, 0);
        let first_readiness = validate_readiness(&db, first.receipt).await.unwrap();
        assert_eq!(
            first_readiness.eligibility(),
            &DormantGitRepositoryR1Eligibility::Eligible
        );

        let replay = run_catchup_with_proof(&db, TestDormantGitRepositoryExclusionProof::new())
            .await
            .unwrap();
        assert_eq!(replay.stats, DormantGitRepositoryCatchupStats::default());
        let replay_readiness = validate_readiness(&db, replay.receipt).await.unwrap();
        assert_eq!(
            replay_readiness.eligibility(),
            &DormantGitRepositoryR1Eligibility::Eligible
        );
        assert!(replay_readiness.has_fresh_root_from(&first_readiness));
        assert_eq!(
            source_snapshot(&db, &canonical_path, &conversation_id).await,
            before_source,
            "candidate catch-up may derive shadows but must not mutate historical sources"
        );

        let root = replay_readiness.unit4_binding();
        let census = DormantGitRepositorySourceCensusAttestation {
            root: root.clone(),
            revision: census_revision,
            content_digest: census_content_digest,
            shadow_reference_count,
            project_authority_path_count,
            clean: true,
        };
        let compatibility = DormantGitRepositoryOldBinaryCompatibilityAttestation {
            root: root.clone(),
            historical_sha: historical_sha.clone(),
            candidate_schema_digest: replay_readiness.schema.compiled_migration_digest.clone(),
            old_source_unchanged: true,
            catchup_exact: true,
            replay_idempotent: true,
            fresh_readiness_root: true,
            passed: true,
        };
        let rollback = DormantGitRepositoryRollbackAttestation {
            root,
            posture: DormantGitRepositoryRollbackPosture::AdditiveSchemaRetainedDestructiveDownMigrationProhibited,
            additive_schema_retained: shadow_counts(&db).await.len() == 4,
            old_source_unchanged: true,
        };
        let evidence = DormantGitRepositoryCompleteCompatibilityEvidence::new(
            replay_readiness,
            census,
            compatibility,
            rollback,
            &format!("{canonical_path}:{conversation_id}"),
        )
        .unwrap();
        let (
            candidate_sha,
            candidate_schema_digest,
            evidence_historical_sha,
            evidence_census_revision,
            evidence_shadow_reference_count,
            evidence_project_authority_path_count,
            complete_eligibility,
            integrity_digest,
        ) = evidence.artifact_fields();
        let artifact = CompatibilityFinalizerArtifact {
            candidate_sha: candidate_sha.to_string(),
            candidate_schema_digest: candidate_schema_digest.to_string(),
            historical_sha: evidence_historical_sha.to_string(),
            census_revision: evidence_census_revision.to_string(),
            census_content_digest: evidence.census_digest().to_string(),
            shadow_reference_count: evidence_shadow_reference_count,
            project_authority_path_count: evidence_project_authority_path_count,
            rollback_posture: evidence.rollback_posture().to_string(),
            complete_eligibility,
            integrity_digest: integrity_digest.to_string(),
            catchup_inserted_repositories: first.stats.inserted_git_repositories,
            catchup_inserted_attachments: first.stats.inserted_work_scope_attachments,
        };
        fs::write(
            &artifact_path,
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
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
