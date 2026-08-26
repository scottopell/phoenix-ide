use chrono::Utc;
use phoenix_core::runtime_resource::{
    RuntimeResourceAdmission, RuntimeResourceAdmissionError, RuntimeResourceInstanceId,
    RuntimeResourceInstanceState, RuntimeResourceKind,
};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResourceInstance {
    pub admission: RuntimeResourceAdmission,
    pub state: RuntimeResourceInstanceState,
}

impl Database {
    /// Persists one exact admission before the resource becomes available to
    /// callers. Replaying the same immutable instance id is idempotent; a
    /// conflicting identity is rejected rather than overwritten.
    ///
    /// # Errors
    /// Returns an error for invalid admission shape, missing scope, conflicting
    /// immutable instance identity, or persistence failure.
    pub async fn admit_runtime_resource_instance(
        &self,
        admission: RuntimeResourceAdmission,
    ) -> DbResult<RuntimeResourceInstance> {
        let admission = admission.validate().map_err(admission_error)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = select_instance(&mut *tx, &admission.instance_id).await?;
        if let Some(existing) = existing {
            if existing.admission == admission {
                tx.commit().await?;
                return Ok(existing);
            }
            return Err(DbError::CloseFoundationConflict(format!(
                "runtime resource instance {} has a conflicting immutable admission",
                admission.instance_id
            )));
        }
        sqlx::query(
            "INSERT INTO runtime_resource_instances (
                 instance_id, work_scope_id, resource_kind, state, launch_uuid,
                 pid, process_birth, pgid, tmux_socket_path, tmux_server_token,
                 browser_session_key, browser_audience, browser_profile_path, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'live', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        )
        .bind(admission.instance_id.as_str())
        .bind(admission.scope.as_str())
        .bind(admission.kind.as_str())
        .bind(&admission.launch_uuid)
        .bind(admission.pid.map(i64::from))
        .bind(admission.process_birth.map(|birth| birth.to_string()))
        .bind(admission.pgid.map(i64::from))
        .bind(&admission.tmux_socket_path)
        .bind(&admission.tmux_server_token)
        .bind(&admission.browser_session_key)
        .bind(&admission.browser_audience)
        .bind(&admission.browser_profile_path)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RuntimeResourceInstance {
            admission,
            state: RuntimeResourceInstanceState::Live,
        })
    }

    /// # Errors
    /// Returns a persistence or decoding error.
    pub async fn runtime_resource_instance(
        &self,
        instance_id: &RuntimeResourceInstanceId,
    ) -> DbResult<Option<RuntimeResourceInstance>> {
        select_instance(&self.pool, instance_id).await
    }

    /// State is the only mutable fact of a runtime instance; all authority
    /// fields are protected by the migration trigger.
    ///
    /// # Errors
    /// Returns an error when the instance does not exist or persistence fails.
    pub async fn set_runtime_resource_instance_state(
        &self,
        instance_id: &RuntimeResourceInstanceId,
        state: RuntimeResourceInstanceState,
    ) -> DbResult<()> {
        let updated = sqlx::query(
            "UPDATE runtime_resource_instances SET state = ?1, updated_at = ?2 WHERE instance_id = ?3",
        )
        .bind(state.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(instance_id.as_str())
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::CloseFoundationNotFound(format!(
                "runtime resource instance {instance_id}"
            )));
        }
        Ok(())
    }
}

fn admission_error(error: RuntimeResourceAdmissionError) -> DbError {
    DbError::CloseFoundationPrecondition(error.to_string())
}

async fn select_instance<'e, E>(
    executor: E,
    instance_id: &RuntimeResourceInstanceId,
) -> DbResult<Option<RuntimeResourceInstance>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(
        "SELECT instance_id, work_scope_id, resource_kind, state, launch_uuid,
                pid, process_birth, pgid, tmux_socket_path, tmux_server_token,
                browser_session_key, browser_audience, browser_profile_path
         FROM runtime_resource_instances WHERE instance_id = ?1",
    )
    .bind(instance_id.as_str())
    .fetch_optional(executor)
    .await?;
    row.as_ref().map(decode_instance).transpose()
}

fn decode_instance(row: &sqlx::sqlite::SqliteRow) -> DbResult<RuntimeResourceInstance> {
    let kind_raw: String = row.try_get("resource_kind")?;
    let state_raw: String = row.try_get("state")?;
    let instance_id = RuntimeResourceInstanceId::parse(row.try_get::<String, _>("instance_id")?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let scope =
        phoenix_core::work_scope::WorkScopeId::parse(row.try_get::<String, _>("work_scope_id")?)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
    let kind = RuntimeResourceKind::parse(&kind_raw).ok_or_else(|| {
        DbError::Serialization(format!("unknown runtime resource kind {kind_raw}"))
    })?;
    let state = RuntimeResourceInstanceState::parse(&state_raw).ok_or_else(|| {
        DbError::Serialization(format!("unknown runtime resource state {state_raw}"))
    })?;
    let process_birth = row
        .try_get::<Option<String>, _>("process_birth")?
        .map(|value| {
            value
                .parse::<u128>()
                .map_err(|error| DbError::Serialization(error.to_string()))
        })
        .transpose()?;
    let pid = row
        .try_get::<Option<i64>, _>("pid")?
        .map(|value| {
            u32::try_from(value).map_err(|error| DbError::Serialization(error.to_string()))
        })
        .transpose()?;
    let process_group = row
        .try_get::<Option<i64>, _>("pgid")?
        .map(|value| {
            i32::try_from(value).map_err(|error| DbError::Serialization(error.to_string()))
        })
        .transpose()?;
    let admission = RuntimeResourceAdmission {
        instance_id,
        scope,
        kind,
        launch_uuid: row.try_get("launch_uuid")?,
        pid,
        process_birth,
        pgid: process_group,
        tmux_socket_path: row.try_get("tmux_socket_path")?,
        tmux_server_token: row.try_get("tmux_server_token")?,
        browser_session_key: row.try_get("browser_session_key")?,
        browser_audience: row.try_get("browser_audience")?,
        browser_profile_path: row.try_get("browser_profile_path")?,
    }
    .validate()
    .map_err(admission_error)?;
    Ok(RuntimeResourceInstance { admission, state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;

    #[tokio::test]
    async fn admission_is_idempotent_and_identity_is_immutable() {
        let db = Database::open_in_memory().await.unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (id, authority_kind, lifecycle, created_at, updated_at)
             VALUES ('runtime-resource-scope', 'work', 'active', ?1, ?1)",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse("runtime-resource-scope").unwrap();
        let admission = RuntimeResourceAdmission {
            instance_id: RuntimeResourceInstanceId::parse("instance-1").unwrap(),
            scope,
            kind: RuntimeResourceKind::Pty,
            launch_uuid: "launch-1".into(),
            pid: Some(42),
            process_birth: Some(99),
            pgid: None,
            tmux_socket_path: None,
            tmux_server_token: None,
            browser_session_key: None,
            browser_audience: None,
            browser_profile_path: None,
        };
        assert_eq!(
            db.admit_runtime_resource_instance(admission.clone())
                .await
                .unwrap()
                .state,
            RuntimeResourceInstanceState::Live
        );
        assert_eq!(
            db.admit_runtime_resource_instance(admission.clone())
                .await
                .unwrap()
                .admission,
            admission
        );
        let mut conflict = admission.clone();
        conflict.launch_uuid = "other".into();
        assert!(db.admit_runtime_resource_instance(conflict).await.is_err());
        db.set_runtime_resource_instance_state(
            &admission.instance_id,
            RuntimeResourceInstanceState::Retired,
        )
        .await
        .unwrap();
        assert_eq!(
            db.runtime_resource_instance(&admission.instance_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            RuntimeResourceInstanceState::Retired
        );
    }
}
