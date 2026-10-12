use phoenix_core::{domain::close::CloseRunOrdinal, work_scope::WorkScopeId};

use sqlx::{Row, Sqlite, Transaction};

use crate::{CloseCleanupFailureResource, CloseCleanupResourceDisposition, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFailureRemainingResource {
    pub scope: WorkScopeId,
    pub resource_kind: String,
    pub identity_kind: String,
    pub identity_codec: String,
    pub identity_value: String,
    pub disposition: CloseCleanupResourceDisposition,
}

impl From<CloseCleanupFailureResource> for CloseFailureRemainingResource {
    fn from(resource: CloseCleanupFailureResource) -> Self {
        Self {
            scope: resource.scope,
            resource_kind: resource.resource.kind().as_str().into(),
            identity_kind: resource.resource.identity().identity_kind().into(),
            identity_codec: resource.resource.identity().codec().into(),
            identity_value: resource.resource.identity().value(),
            disposition: resource.disposition,
        }
    }
}

pub(super) async fn remaining_resources(
    pool: &sqlx::SqlitePool,
    failure_occurrence_id: &str,
) -> DbResult<Vec<CloseFailureRemainingResource>> {
    sqlx::query(
        "SELECT scope, resource_kind, identity_kind, identity_codec, identity_value, disposition
         FROM close_cleanup_failure_resources WHERE failure_occurrence_id = ?1 ORDER BY ordinal",
    )
    .bind(failure_occurrence_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok(CloseFailureRemainingResource {
            scope: WorkScopeId::parse(row.try_get::<String, _>("scope")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            resource_kind: row.try_get("resource_kind")?,
            identity_kind: row.try_get("identity_kind")?,
            identity_codec: row.try_get("identity_codec")?,
            identity_value: row.try_get("identity_value")?,
            disposition: match row.try_get::<&str, _>("disposition")? {
                "failed" => CloseCleanupResourceDisposition::Failed,
                "residual" => CloseCleanupResourceDisposition::Residual,
                "unattempted" => CloseCleanupResourceDisposition::Unattempted,
                "unknown" => CloseCleanupResourceDisposition::Unknown,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown cleanup resource disposition {other}"
                    )))
                }
            },
        })
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandatoryCloseFailureSubject {
    Attempt,
    Resource {
        scope: String,
        resource_kind: String,
        identity_kind: String,
        identity_codec: String,
        identity_value: String,
    },
}

#[derive(Debug, Clone)]
pub enum WatchEventRoute {
    Subscription,
    MandatoryCloseFailure {
        run_ordinal: CloseRunOrdinal,
        remaining_resources: Vec<CloseFailureRemainingResource>,
        subject: MandatoryCloseFailureSubject,
        detail: String,
        stop: CloseFailureStop,
    },
}

#[derive(Debug, Clone)]
pub enum CloseFailureStop {
    ConversationAndProcessesStopped { confirmed_at_unix_us: i64 },
    ShutdownUncertain,
}

pub(super) fn decode_route(row: &sqlx::sqlite::SqliteRow) -> DbResult<WatchEventRoute> {
    let route: String = row.try_get("route_kind")?;
    match route.as_str() {
        "subscription" => Ok(WatchEventRoute::Subscription),
        "mandatory_close_failure" => {
            let certainty: String = row.try_get("stop_certainty")?;
            let confirmed: Option<i64> = row.try_get("stop_confirmed_at_unix_us")?;
            let stop = match (certainty.as_str(), confirmed) {
                ("conversation_and_processes_stopped", Some(confirmed_at_unix_us))
                    if confirmed_at_unix_us >= 0 =>
                {
                    CloseFailureStop::ConversationAndProcessesStopped {
                        confirmed_at_unix_us,
                    }
                }
                ("shutdown_uncertain", None) => CloseFailureStop::ShutdownUncertain,
                _ => {
                    return Err(DbError::Serialization(
                        "invalid Close stop certainty".into(),
                    ))
                }
            };
            let authority_kind: String = row.try_get("authority_kind")?;
            let subject = if authority_kind == "attempt_interrupted" {
                MandatoryCloseFailureSubject::Attempt
            } else {
                MandatoryCloseFailureSubject::Resource {
                    scope: row.try_get("scope")?,
                    resource_kind: row.try_get("resource_kind")?,
                    identity_kind: row.try_get("identity_kind")?,
                    identity_codec: row.try_get("identity_codec")?,
                    identity_value: row.try_get("identity_value")?,
                }
            };
            Ok(WatchEventRoute::MandatoryCloseFailure {
                run_ordinal: CloseRunOrdinal::parse(row.try_get("cleanup_run_ordinal")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                remaining_resources: Vec::new(),
                subject,
                detail: row.try_get("detail")?,
                stop,
            })
        }
        _ => Err(DbError::Serialization("invalid watch event route".into())),
    }
}

/// Append an already-persisted Close failure to the existing Global outbox.
/// The caller owns the transaction and wakes delivery only after commit.
///
/// # Errors
/// Returns an error when the failure is absent or the database operation fails.
pub async fn append_mandatory_close_failure_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    failure_occurrence_id: &str,
) -> DbResult<bool> {
    let roots: Vec<String> = sqlx::query_scalar(
        "SELECT root.id FROM close_cleanup_failures f
         JOIN conversations root ON root.product_conversation_id = f.source_product_conversation_id
         WHERE f.failure_occurrence_id = ?1 AND root.runtime_role = 'user'
           AND root.parent_conversation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM conversations predecessor
             WHERE predecessor.product_conversation_id = root.product_conversation_id
               AND predecessor.continued_in_conv_id = root.id)",
    )
    .bind(failure_occurrence_id)
    .fetch_all(&mut **tx)
    .await?;
    let [source_transcript_id] = roots.as_slice() else {
        return Err(DbError::Serialization(
            "Close failure must have exactly one authoritative product root".into(),
        ));
    };
    let result = sqlx::query(
        "INSERT INTO coordinator_watch_events
         (event_id, route_kind, watch_id, mandatory_failure_occurrence_id,
          mandatory_source_product_id, source_occurrence_kind, source_occurrence_id,
          source_generation, source_transcript_id, terminal_kind, terminal_reason, occurred_at_us)
         SELECT f.failure_occurrence_id, 'mandatory_close_failure', NULL, f.failure_occurrence_id,
                f.source_product_conversation_id, 'close_cleanup_failure', f.failure_occurrence_id,
                0, ?2, 'cleanup_failed', f.reason, f.occurred_at_us
         FROM close_cleanup_failures f JOIN product_conversations p
           ON p.id = f.source_product_conversation_id
         WHERE f.failure_occurrence_id = ?1
         ON CONFLICT(mandatory_failure_occurrence_id) DO NOTHING",
    )
    .bind(failure_occurrence_id)
    .bind(source_transcript_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}
