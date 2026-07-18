use super::{
    wake_profile, CommitOutcome, CommitTransitionPlanCas, CreateWorkflowWithExternalAcceptance,
    DbError, DbResult, LocalCodec, LocalEffectDecl, WorkflowRepository, WorkflowTx,
};
use phoenix_workflow::{
    wake_profile::{
        self as wake_types, ObserveHandleIntent, WakeRegistrationEvent, WakeRegistrationIntent,
        WakeRegistrationReceipt, WakeRegistrationSnapshot, WakeResourceIdentity,
        REGISTRATION_EFFECT_ID,
    },
    EffectRole, EffectStatus, ErasedAcceptanceProfile, Generation, ProfileRef, Timestamp,
    TransitionId, Version, WorkflowId, WorkflowStatus,
};
use serde::Serialize;
use sqlx::Row;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
static FAIL_AFTER_CANONICAL_TRANSITION: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub fn fail_after_canonical_transition_once(workflow_id: WorkflowId) {
    FAIL_AFTER_CANONICAL_TRANSITION.store(workflow_id.0, Ordering::SeqCst);
}

#[cfg(test)]
fn maybe_fail_after_canonical_transition(workflow_id: WorkflowId) -> DbResult<()> {
    if FAIL_AFTER_CANONICAL_TRANSITION
        .compare_exchange(workflow_id.0, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        return Err(DbError::Serialization(
            "test failpoint after canonical transition".to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_after_canonical_transition(_workflow_id: WorkflowId) {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBindingRecord {
    pub workflow_id: WorkflowId,
    pub conversation_id: String,
    pub contract_id: String,
    pub profile: ProfileRef,
    pub registration_scope: wake_types::WorkScopeIdentity,
    pub resource: WakeResourceIdentity,
    pub registering_tool_use_id: String,
    pub expires_at: Timestamp,
    pub prepared_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationOutcome {
    Registered {
        workflow_id: WorkflowId,
        receipt: WakeRegistrationReceipt,
    },
    Replayed {
        workflow_id: WorkflowId,
        receipt: WakeRegistrationReceipt,
    },
    Conflict,
}

#[derive(Debug, Clone)]
pub struct WakeRepository {
    workflow_repo: WorkflowRepository,
}

impl WakeRepository {
    #[must_use]
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self {
            workflow_repo: WorkflowRepository::new(pool),
        }
    }

    pub async fn register(
        &self,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        workflow_id: WorkflowId,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        for _ in 0..20 {
            match self
                .register_once(input, prepared_fingerprint, workflow_id, now)
                .await
            {
                Err(DbError::Sqlx(sqlx::Error::Database(error)))
                    if error.code().as_deref() == Some("5") =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
        self.register_once(input, prepared_fingerprint, workflow_id, now)
            .await
    }

    async fn register_once(
        &self,
        input: &WakeRegistrationIntent,
        prepared_fingerprint: &str,
        workflow_id: WorkflowId,
        now: Timestamp,
    ) -> DbResult<WakeRegistrationOutcome> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let existing = fetch_existing_binding_tx(&mut tx, input).await?;
        if let Some(existing) = existing {
            tx.commit().await?;
            return Ok(if existing.prepared_fingerprint == prepared_fingerprint {
                WakeRegistrationOutcome::Replayed {
                    workflow_id: existing.workflow_id,
                    receipt: replay_receipt(&existing),
                }
            } else {
                WakeRegistrationOutcome::Conflict
            });
        }

        let snapshot = WakeRegistrationSnapshot {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            registered: true,
            terminal: None,
            runtime_availability: wake_profile::RuntimeAvailability::Pending,
        };
        let observe_intent = ObserveHandleIntent {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            expires_at: input.expires_at,
        };
        let acceptance = ErasedAcceptanceProfile::from_parts(
            wake_profile::profile(),
            wake_profile::acceptance_profile().supported_codecs.clone(),
            true,
            false,
        );
        let create = CreateWorkflowWithExternalAcceptance {
            workflow_id,
            profile: wake_profile::profile(),
            acceptance,
            target_scope: phoenix_workflow::ScopeId::new(format!(
                "wake:{}:{}",
                input.conversation_id, input.contract_id
            ))
            .ok_or_else(|| DbError::Serialization("empty wake acceptance identity".to_string()))?,
            idempotency_key: phoenix_workflow::NonEmptyExternalKey::new(format!(
                "wake:{}:{}",
                input.conversation_id,
                resource_key(&input.resource)
            ))
            .ok_or_else(|| DbError::Serialization("empty wake acceptance identity".to_string()))?,
            intent_fingerprint: prepared_fingerprint.to_string(),
            snapshot_codec: wake_profile::snapshot_codec(),
            snapshot_payload: json_blob(&snapshot)?,
            receipt_handle: resource_key(&input.resource).into_bytes(),
            disposition_handle: input.contract_id.clone().into_bytes(),
            now,
        };
        tx.insert_workflow(&create).await?;

        let receipt = WakeRegistrationReceipt {
            contract_id: input.contract_id.clone(),
            resource: input.resource.clone(),
            expires_at: input.expires_at,
            registering_tool_use_id: input.registering_tool_use_id.clone(),
        };
        let plan = CommitTransitionPlanCas {
            workflow_id,
            expected_version: Version(0),
            transition_id: TransitionId(1),
            generation: Generation(0),
            next_status: WorkflowStatus::Active,
            event_codec: local_codec(&wake_profile::event_codec()),
            event_payload: json_blob(&WakeRegistrationEvent::Registered)?,
            next_snapshot_codec: local_codec(&wake_profile::snapshot_codec()),
            next_snapshot_payload: json_blob(&snapshot)?,
            committed_at: now,
            effects: vec![LocalEffectDecl {
                effect_id: REGISTRATION_EFFECT_ID,
                declared_workflow_version: Version(1),
                family: "wake.observe".to_string(),
                kind: "observe_handle".to_string(),
                intent_codec: local_codec(&wake_profile::intent_codec()),
                intent_payload: json_blob(&observe_intent)?,
                generation: Generation(0),
                role: EffectRole::Required,
                capability: phoenix_workflow::ExecutionCapability::ReclaimableObservation,
                next_eligible_at: None,
                destructive_resource: None,
                status: EffectStatus::Eligible,
            }],
            dependencies: vec![],
            barriers: vec![],
            barrier_members: vec![],
            deliveries: vec![],
            schedules: vec![],
        };
        match tx.commit_transition_plan(&plan).await? {
            CommitOutcome::Committed => {}
            CommitOutcome::VersionConflict
            | CommitOutcome::InvalidPlan
            | CommitOutcome::UnsupportedCodec => {
                tx.rollback().await?;
                return Err(DbError::Serialization(
                    "wake registration transition was rejected".to_string(),
                ));
            }
        }
        #[cfg(test)]
        maybe_fail_after_canonical_transition(workflow_id)?;
        #[cfg(not(test))]
        maybe_fail_after_canonical_transition(workflow_id);
        insert_binding_tx(&mut tx, workflow_id, input, prepared_fingerprint, now).await?;
        tx.commit().await?;
        Ok(WakeRegistrationOutcome::Registered {
            workflow_id,
            receipt,
        })
    }

    pub async fn fetch_binding(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WakeBindingRecord>> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let row = fetch_binding_by_workflow_tx(&mut tx, workflow_id).await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn reload_binding(
        &self,
        workflow_id: WorkflowId,
    ) -> DbResult<Option<WakeBindingRecord>> {
        self.fetch_binding(workflow_id).await
    }
}

fn local_codec(codec: &phoenix_workflow::CodecRef) -> LocalCodec {
    LocalCodec {
        family: codec.family.to_string(),
        version: codec.version,
    }
}

fn json_blob<T: Serialize>(value: &T) -> DbResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| DbError::Serialization(e.to_string()))
}

fn resource_key(resource: &WakeResourceIdentity) -> String {
    match resource {
        WakeResourceIdentity::Bash(identity) => format!("bash:{}", identity.handle_id),
        WakeResourceIdentity::TmuxWindow(identity) => {
            format!("tmux:{}:{}", identity.server_generation, identity.window_id)
        }
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn replay_receipt(existing: &WakeBindingRecord) -> WakeRegistrationReceipt {
    WakeRegistrationReceipt {
        contract_id: existing.contract_id.clone(),
        resource: existing.resource.clone(),
        expires_at: existing.expires_at,
        registering_tool_use_id: existing.registering_tool_use_id.clone(),
    }
}

async fn fetch_existing_binding_tx(
    tx: &mut WorkflowTx<'_>,
    input: &WakeRegistrationIntent,
) -> DbResult<Option<WakeBindingRecord>> {
    let row = sqlx::query(
        "SELECT workflow_id, conversation_id, contract_id, profile_kind, profile_version,
                scope_kind, scope_stable_key, resource_kind, bash_handle_id,
                tmux_server_generation, tmux_window_id, registering_tool_use_id,
                expires_at, prepared_fingerprint
         FROM wake_bindings
         WHERE profile_kind = 'wake' AND profile_version = ?1 AND conversation_id = ?2
           AND contract_id = ?3 AND resource_kind = ?4
           AND COALESCE(bash_handle_id, '') = ?5
           AND COALESCE(tmux_server_generation, '') = ?6
           AND COALESCE(tmux_window_id, '') = ?7",
    )
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource).unwrap_or_default())
    .bind(tmux_server_generation(&input.resource).unwrap_or_default())
    .bind(tmux_window_id(&input.resource).unwrap_or_default())
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

async fn fetch_binding_by_workflow_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<Option<WakeBindingRecord>> {
    let row = sqlx::query(
        "SELECT workflow_id, conversation_id, contract_id, profile_kind, profile_version,
                scope_kind, scope_stable_key, resource_kind, bash_handle_id,
                tmux_server_generation, tmux_window_id, registering_tool_use_id,
                expires_at, prepared_fingerprint
         FROM wake_bindings WHERE workflow_id = ?1",
    )
    .bind(i64::try_from(workflow_id.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .fetch_optional(&mut *tx.tx)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

async fn insert_binding_tx(
    tx: &mut WorkflowTx<'_>,
    workflow_id: WorkflowId,
    input: &WakeRegistrationIntent,
    prepared_fingerprint: &str,
    now: Timestamp,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO wake_bindings (
            workflow_id, conversation_id, contract_id, profile_kind, profile_version,
            scope_kind, scope_stable_key, resource_kind, bash_handle_id,
            tmux_server_generation, tmux_window_id, registering_tool_use_id,
            expires_at, prepared_fingerprint, observe_effect_id, created_at
         ) VALUES (?1, ?2, ?3, 'wake', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )
    .bind(i64::try_from(workflow_id.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(&input.conversation_id)
    .bind(&input.contract_id)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(scope_kind_str(&input.registration_scope))
    .bind(&input.registration_scope.stable_key)
    .bind(resource_kind_str(&input.resource))
    .bind(bash_handle_id(&input.resource))
    .bind(tmux_server_generation(&input.resource))
    .bind(tmux_window_id(&input.resource))
    .bind(&input.registering_tool_use_id)
    .bind(i64::try_from(input.expires_at.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .bind(prepared_fingerprint)
    .bind(
        i64::try_from(REGISTRATION_EFFECT_ID.0)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
    )
    .bind(i64::try_from(now.0).map_err(|e| DbError::Serialization(e.to_string()))?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

fn binding_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeBindingRecord> {
    Ok(WakeBindingRecord {
        workflow_id: WorkflowId(
            u64::try_from(row.get::<i64, _>("workflow_id"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        ),
        conversation_id: row.get("conversation_id"),
        contract_id: row.get("contract_id"),
        profile: ProfileRef {
            profile_kind: row.get("profile_kind"),
            profile_version: u32::try_from(row.get::<i64, _>("profile_version"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        },
        registration_scope: wake_types::WorkScopeIdentity {
            kind: match row.get::<String, _>("scope_kind").as_str() {
                "Conversation" => wake_types::WorkScopeKind::Conversation,
                "Worktree" => wake_types::WorkScopeKind::Worktree,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown scope kind: {other}"
                    )))
                }
            },
            stable_key: row.get("scope_stable_key"),
        },
        resource: resource_from_row(row)?,
        registering_tool_use_id: row.get("registering_tool_use_id"),
        expires_at: Timestamp(
            u64::try_from(row.get::<i64, _>("expires_at"))
                .map_err(|e| DbError::Serialization(e.to_string()))?,
        ),
        prepared_fingerprint: row.get("prepared_fingerprint"),
    })
}

fn resource_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<WakeResourceIdentity> {
    match row.get::<String, _>("resource_kind").as_str() {
        "Bash" => Ok(WakeResourceIdentity::Bash(
            wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: match row.get::<String, _>("scope_kind").as_str() {
                        "Conversation" => wake_types::WorkScopeKind::Conversation,
                        "Worktree" => wake_types::WorkScopeKind::Worktree,
                        other => {
                            return Err(DbError::Serialization(format!(
                                "unknown scope kind: {other}"
                            )))
                        }
                    },
                    stable_key: row.get("scope_stable_key"),
                },
                handle_id: row.get::<String, _>("bash_handle_id"),
            },
        )),
        "TmuxWindow" => Ok(WakeResourceIdentity::TmuxWindow(
            wake_types::TmuxResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: match row.get::<String, _>("scope_kind").as_str() {
                        "Conversation" => wake_types::WorkScopeKind::Conversation,
                        "Worktree" => wake_types::WorkScopeKind::Worktree,
                        other => {
                            return Err(DbError::Serialization(format!(
                                "unknown scope kind: {other}"
                            )))
                        }
                    },
                    stable_key: row.get("scope_stable_key"),
                },
                server_generation: row.get::<String, _>("tmux_server_generation"),
                window_id: row.get::<String, _>("tmux_window_id"),
            },
        )),
        other => Err(DbError::Serialization(format!(
            "unknown resource kind: {other}"
        ))),
    }
}

fn scope_kind_str(scope: &wake_types::WorkScopeIdentity) -> &'static str {
    match scope.kind {
        wake_types::WorkScopeKind::Conversation => "Conversation",
        wake_types::WorkScopeKind::Worktree => "Worktree",
    }
}

fn resource_kind_str(resource: &WakeResourceIdentity) -> &'static str {
    match resource {
        WakeResourceIdentity::Bash(_) => "Bash",
        WakeResourceIdentity::TmuxWindow(_) => "TmuxWindow",
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn bash_handle_id(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::Bash(identity) => Some(identity.handle_id.clone()),
        WakeResourceIdentity::TmuxWindow(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn tmux_server_generation(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => Some(identity.server_generation.clone()),
        WakeResourceIdentity::Bash(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

fn tmux_window_id(resource: &WakeResourceIdentity) -> Option<String> {
    match resource {
        WakeResourceIdentity::TmuxWindow(identity) => Some(identity.window_id.clone()),
        WakeResourceIdentity::Bash(_) => None,
        WakeResourceIdentity::Subagent(_) => unreachable!("subagent wake bindings not implemented"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_repo_schema(pool: &sqlx::SqlitePool) {
        sqlx::query("CREATE TABLE conversations (id TEXT PRIMARY KEY, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, steering_queue TEXT NOT NULL DEFAULT '[]', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT UNIQUE, conversation_id TEXT NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, sequence_id INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL DEFAULT '2025-01-01')").execute(pool).await.unwrap();
        run_pending_migrations(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO conversations (id) VALUES ('conv-1')")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn open_repo_pair() -> (tempfile::TempDir, WakeRepository, WakeRepository) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wake.db");
        let url = format!("sqlite://{}", path.display());
        let open = || async {
            let opts = SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5));
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await
                .unwrap();
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await
                .unwrap();
            if sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conversations'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
                == 0
            {
                setup_repo_schema(&pool).await;
            }
            WakeRepository::new(pool)
        };
        (dir, open().await, open().await)
    }

    fn intent() -> WakeRegistrationIntent {
        WakeRegistrationIntent {
            contract_id: "contract-1".into(),
            conversation_id: "conv-1".into(),
            registration_scope: wake_types::WorkScopeIdentity {
                kind: wake_types::WorkScopeKind::Conversation,
                stable_key: "conv-1".into(),
            },
            resource: WakeResourceIdentity::Bash(wake_types::BashResourceIdentity {
                work_scope: wake_types::WorkScopeIdentity {
                    kind: wake_types::WorkScopeKind::Conversation,
                    stable_key: "conv-1".into(),
                },
                handle_id: "b-1".into(),
            }),
            registering_tool_use_id: "tool-1".into(),
            registered_at: Timestamp(10),
            expires_at: Timestamp(100),
        }
    }

    #[tokio::test]
    async fn duplicate_concurrent_registration_replays_single_winner() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let (left, right) = tokio::join!(
            first.register(&input, "fp-1", WorkflowId(100), Timestamp(10)),
            second.register(&input, "fp-1", WorkflowId(101), Timestamp(10))
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeRegistrationOutcome::Registered { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, WakeRegistrationOutcome::Replayed { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn mutable_input_conflict() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        assert!(matches!(
            repo.register(&input, "fp-1", WorkflowId(102), Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Registered { .. }
        ));
        assert_eq!(
            repo.register(&input, "fp-2", WorkflowId(103), Timestamp(10))
                .await
                .unwrap(),
            WakeRegistrationOutcome::Conflict
        );
    }

    #[tokio::test]
    async fn failpoint_rolls_back_everything() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = intent();
        fail_after_canonical_transition_once(WorkflowId(104));
        let err = repo
            .register(&input, "fp-1", WorkflowId(104), Timestamp(10))
            .await;
        assert!(err.is_err());
        assert!(repo.fetch_binding(WorkflowId(104)).await.unwrap().is_none());
        assert_eq!(
            repo.workflow_repo
                .fetch_workflow_head(WorkflowId(104))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn restart_reload_finds_binding() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = intent();
        let registered = first
            .register(&input, "fp-1", WorkflowId(105), Timestamp(10))
            .await
            .unwrap();
        assert!(matches!(
            registered,
            WakeRegistrationOutcome::Registered { .. }
        ));
        let binding = second
            .reload_binding(WorkflowId(105))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(binding.contract_id, "contract-1");
        assert_eq!(binding.prepared_fingerprint, "fp-1");
    }
}
