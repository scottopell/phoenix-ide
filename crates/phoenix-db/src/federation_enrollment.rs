use std::str::FromStr;

use chrono::Utc;
use phoenix_core::domain::instance_identity::{FederationCredentialVerifier, InstanceId};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationEnrollment {
    pub caller_instance_id: InstanceId,
    pub caller_display_name: String,
    pub revoked_at_us: Option<i64>,
}

fn decode_enrollment(row: &sqlx::sqlite::SqliteRow) -> DbResult<FederationEnrollment> {
    let caller_id: String = row.try_get("caller_instance_id")?;
    Ok(FederationEnrollment {
        caller_instance_id: InstanceId::from_str(&caller_id)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        caller_display_name: row.try_get("caller_display_name")?,
        revoked_at_us: row.try_get("revoked_at_us")?,
    })
}

#[derive(Clone, Copy)]
enum EnrollmentCommit {
    Normal,
    #[cfg(test)]
    CommittedAckLost,
}

impl EnrollmentCommit {
    async fn execute(self, tx: sqlx::Transaction<'_, sqlx::Sqlite>) -> DbResult<()> {
        tx.commit().await?;
        #[cfg(test)]
        if matches!(self, Self::CommittedAckLost) {
            return Err(DbError::Serialization(
                "injected committed enrollment acknowledgement loss".into(),
            ));
        }
        Ok(())
    }
}

fn persisted_timestamp(observed_clock_us: i64, previous_created_at: Option<i64>) -> DbResult<i64> {
    if observed_clock_us < 0 {
        return Err(DbError::Serialization(
            "federation enrollment clock is before the Unix epoch".into(),
        ));
    }
    Ok(observed_clock_us.max(previous_created_at.unwrap_or(0)))
}

impl Database {
    /// Replace any active receiver-issued credential for one caller instance.
    ///
    /// # Errors
    /// Returns a database error if replacement cannot be committed atomically.
    pub async fn replace_federation_enrollment(
        &self,
        caller_instance_id: InstanceId,
        caller_display_name: &str,
        credential_verifier: &FederationCredentialVerifier,
    ) -> DbResult<FederationEnrollment> {
        self.replace_federation_enrollment_with_commit(
            caller_instance_id,
            caller_display_name,
            credential_verifier,
            EnrollmentCommit::Normal,
            None,
        )
        .await
    }

    async fn replace_federation_enrollment_with_commit(
        &self,
        caller_instance_id: InstanceId,
        caller_display_name: &str,
        credential_verifier: &FederationCredentialVerifier,
        commit: EnrollmentCommit,
        observed_clock_us: Option<i64>,
    ) -> DbResult<FederationEnrollment> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous_created_at: Option<i64> = sqlx::query_scalar(
            "SELECT created_at_us FROM federation_enrollments
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let now = persisted_timestamp(
            observed_clock_us.unwrap_or_else(|| Utc::now().timestamp_micros()),
            previous_created_at,
        )?;
        sqlx::query(
            "UPDATE federation_enrollments SET revoked_at_us = ?2
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO federation_enrollments
                 (id, caller_instance_id, caller_display_name, credential_verifier, created_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(caller_instance_id.to_string())
        .bind(caller_display_name)
        .bind(credential_verifier.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(
            "SELECT caller_instance_id, caller_display_name, revoked_at_us
             FROM federation_enrollments
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let enrollment = decode_enrollment(&row)?;
        match commit.execute(tx).await {
            Ok(()) => Ok(enrollment),
            Err(commit_error) => match self
                .authenticate_federation_verifier(credential_verifier)
                .await
            {
                Ok(Some(committed))
                    if committed.caller_instance_id == caller_instance_id
                        && committed.caller_display_name == caller_display_name =>
                {
                    Ok(committed)
                }
                Ok(_) => Err(commit_error),
                Err(classification_error) => Err(DbError::Serialization(format!(
                    "enrollment commit failed ({commit_error}); exact verifier classification failed: {classification_error}"
                ))),
            },
        }
    }

    /// Revoke the active receiver-issued credential for one caller instance.
    ///
    /// # Errors
    /// Returns a database error if revocation cannot be committed.
    pub async fn revoke_federation_enrollment(
        &self,
        caller_instance_id: InstanceId,
    ) -> DbResult<bool> {
        self.revoke_federation_enrollment_at(caller_instance_id, None)
            .await
    }

    async fn revoke_federation_enrollment_at(
        &self,
        caller_instance_id: InstanceId,
        observed_clock_us: Option<i64>,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous_created_at: Option<i64> = sqlx::query_scalar(
            "SELECT created_at_us FROM federation_enrollments
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let now = persisted_timestamp(
            observed_clock_us.unwrap_or_else(|| Utc::now().timestamp_micros()),
            previous_created_at,
        )?;
        let result = sqlx::query(
            "UPDATE federation_enrollments SET revoked_at_us = ?2
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() != 0)
    }

    /// Resolve an active credential verifier to its authenticated caller.
    ///
    /// # Errors
    /// Returns a database or serialization error if lookup cannot complete.
    pub async fn authenticate_federation_verifier(
        &self,
        credential_verifier: &FederationCredentialVerifier,
    ) -> DbResult<Option<FederationEnrollment>> {
        let row = sqlx::query(
            "SELECT caller_instance_id, caller_display_name, revoked_at_us
             FROM federation_enrollments
             WHERE credential_verifier = ?1 AND revoked_at_us IS NULL",
        )
        .bind(credential_verifier.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(decode_enrollment).transpose()
    }

    /// Return the active enrollment for a caller instance, if one exists.
    ///
    /// # Errors
    /// Returns a database or serialization error if the enrollment cannot be read.
    pub async fn active_federation_enrollment(
        &self,
        caller_instance_id: InstanceId,
    ) -> DbResult<Option<FederationEnrollment>> {
        let row = sqlx::query(
            "SELECT caller_instance_id, caller_display_name, revoked_at_us
             FROM federation_enrollments
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(decode_enrollment).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> Database {
        let db = Database::open_in_memory().await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        db
    }

    #[tokio::test]
    async fn committed_replacement_is_classified_by_exact_verifier() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"bearer");

        let enrollment = db
            .replace_federation_enrollment_with_commit(
                caller,
                "peer",
                &verifier,
                EnrollmentCommit::CommittedAckLost,
                None,
            )
            .await
            .unwrap();
        assert_eq!(enrollment.caller_instance_id, caller);
        assert!(db
            .authenticate_federation_verifier(&verifier)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn replacement_revokes_previous_credential_atomically() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let first = FederationCredentialVerifier::from_bearer(b"bearer-one");
        let second = FederationCredentialVerifier::from_bearer(b"bearer-two");
        db.replace_federation_enrollment(caller, "peer", &first)
            .await
            .unwrap();
        db.replace_federation_enrollment(caller, "renamed peer", &second)
            .await
            .unwrap();

        assert!(db
            .authenticate_federation_verifier(&first)
            .await
            .unwrap()
            .is_none());
        let active = db
            .authenticate_federation_verifier(&second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.caller_instance_id, caller);
        assert_eq!(active.caller_display_name, "renamed peer");
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM federation_enrollments
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(active_count, 1);
    }

    #[tokio::test]
    async fn failed_replacement_rolls_back_previous_revocation() {
        let db = test_db().await;
        let first_caller = InstanceId::new();
        let second_caller = InstanceId::new();
        let first = FederationCredentialVerifier::from_bearer(b"first-bearer");
        let duplicate = FederationCredentialVerifier::from_bearer(b"duplicate-bearer");
        db.replace_federation_enrollment(first_caller, "first peer", &first)
            .await
            .unwrap();
        db.replace_federation_enrollment(second_caller, "second peer", &duplicate)
            .await
            .unwrap();

        assert!(db
            .replace_federation_enrollment(first_caller, "first peer", &duplicate)
            .await
            .is_err());
        assert_eq!(
            db.authenticate_federation_verifier(&first)
                .await
                .unwrap()
                .unwrap()
                .caller_instance_id,
            first_caller
        );
    }

    #[tokio::test]
    async fn negative_clock_is_rejected_before_monotonic_clamping() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"bearer");
        assert!(db
            .replace_federation_enrollment_with_commit(
                caller,
                "peer",
                &verifier,
                EnrollmentCommit::Normal,
                Some(-1),
            )
            .await
            .is_err());
        assert!(db
            .revoke_federation_enrollment_at(caller, Some(-1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn replacement_survives_clock_rollback() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let first = FederationCredentialVerifier::from_bearer(b"first");
        let second = FederationCredentialVerifier::from_bearer(b"second");
        db.replace_federation_enrollment_with_commit(
            caller,
            "peer",
            &first,
            EnrollmentCommit::Normal,
            Some(100),
        )
        .await
        .unwrap();

        db.replace_federation_enrollment_with_commit(
            caller,
            "peer",
            &second,
            EnrollmentCommit::Normal,
            Some(1),
        )
        .await
        .unwrap();
        assert!(db
            .authenticate_federation_verifier(&second)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn revocation_survives_clock_rollback() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"bearer");
        db.replace_federation_enrollment_with_commit(
            caller,
            "peer",
            &verifier,
            EnrollmentCommit::Normal,
            Some(100),
        )
        .await
        .unwrap();

        assert!(db
            .revoke_federation_enrollment_at(caller, Some(1))
            .await
            .unwrap());
        assert!(db
            .authenticate_federation_verifier(&verifier)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn revocation_prevents_subsequent_authentication() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"bearer");
        db.replace_federation_enrollment(caller, "peer", &verifier)
            .await
            .unwrap();

        assert!(db.revoke_federation_enrollment(caller).await.unwrap());
        assert!(!db.revoke_federation_enrollment(caller).await.unwrap());
        assert!(db
            .authenticate_federation_verifier(&verifier)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .active_federation_enrollment(caller)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn schema_rejects_invalid_timestamp_domains() {
        let db = test_db().await;
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"bearer").to_string();
        assert!(sqlx::query(
            "INSERT INTO federation_enrollments
                 (id, caller_instance_id, caller_display_name, credential_verifier, created_at_us)
             VALUES (?1, ?2, 'peer', ?3, -1)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(caller.to_string())
        .bind(&verifier)
        .execute(db.pool())
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO federation_enrollments
                 (id, caller_instance_id, caller_display_name, credential_verifier, created_at_us)
             VALUES (?1, ?2, 'peer', ?3, 'not-an-integer')",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(caller.to_string())
        .bind(&verifier)
        .execute(db.pool())
        .await
        .is_err());
    }

    #[tokio::test]
    async fn schema_rejects_two_active_credentials_for_one_caller() {
        let db = test_db().await;
        let caller = InstanceId::new();
        db.replace_federation_enrollment(
            caller,
            "peer",
            &FederationCredentialVerifier::from_bearer(b"bearer-one"),
        )
        .await
        .unwrap();

        assert!(sqlx::query(
            "INSERT INTO federation_enrollments
                 (id, caller_instance_id, caller_display_name, credential_verifier, created_at_us)
             VALUES (?1, ?2, 'peer', ?3, 1)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(caller.to_string())
        .bind(FederationCredentialVerifier::from_bearer(b"bearer-two").to_string())
        .execute(db.pool())
        .await
        .is_err());
    }
}
