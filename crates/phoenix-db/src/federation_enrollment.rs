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
        let now = Utc::now().timestamp_micros();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
        tx.commit().await?;
        Ok(enrollment)
    }

    /// Revoke the active receiver-issued credential for one caller instance.
    ///
    /// # Errors
    /// Returns a database error if revocation cannot be committed.
    pub async fn revoke_federation_enrollment(
        &self,
        caller_instance_id: InstanceId,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE federation_enrollments SET revoked_at_us = ?2
             WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
        )
        .bind(caller_instance_id.to_string())
        .bind(Utc::now().timestamp_micros())
        .execute(&self.pool)
        .await?;
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
