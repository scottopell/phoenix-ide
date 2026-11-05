use phoenix_core::domain::coordinator_authority::CoordinatorAuthority;
use sqlx::{Connection, SqliteConnection};

use crate::{workflow::LocalAuthorityResult, Database, DbError, DbResult};

const SETTING_KEY: &str = "coordinator_authority";

async fn read_on(connection: &mut SqliteConnection) -> DbResult<CoordinatorAuthority> {
    let text: Option<String> = sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?1")
        .bind(SETTING_KEY)
        .fetch_optional(connection)
        .await?;
    // owned: instances predating Authority had no standing document; empty is correct.
    CoordinatorAuthority::try_from(text.unwrap_or_default())
        .map_err(|error| DbError::Serialization(error.to_string()))
}

#[derive(Clone, Copy)]
enum AuthorityCommit {
    Acknowledge,
    #[cfg(test)]
    LoseAcknowledgement,
}

impl Database {
    /// Reads the complete current document for authoritative prompt construction.
    ///
    /// # Errors
    /// Returns storage errors and rejects an invalid saved document.
    pub async fn coordinator_authority(&self) -> DbResult<CoordinatorAuthority> {
        let mut connection = self.pool.acquire().await?;
        read_on(&mut connection).await
    }

    /// Reads Authority only for the persisted current Coordinator transcript.
    ///
    /// # Errors
    /// Returns an error for a stale or ineligible source, invalid text, or failed read.
    pub async fn read_coordinator_authority(
        &self,
        source_transcript_id: &str,
    ) -> DbResult<CoordinatorAuthority> {
        let mut tx = self.pool.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, source_transcript_id)
            .await?;
        let authority = read_on(&mut tx).await?;
        tx.commit().await?;
        Ok(authority)
    }

    /// Replaces the complete document after transactionally validating its source.
    ///
    /// # Errors
    /// Acknowledged rollback returns an established error. Lost commit/rollback
    /// acknowledgement returns an unclassified outcome.
    pub async fn set_coordinator_authority(
        &self,
        source_transcript_id: &str,
        authority: &CoordinatorAuthority,
    ) -> LocalAuthorityResult<DbResult<CoordinatorAuthority>> {
        self.set_coordinator_authority_with_commit(
            source_transcript_id,
            authority,
            AuthorityCommit::Acknowledge,
        )
        .await
    }

    async fn set_coordinator_authority_with_commit(
        &self,
        source_transcript_id: &str,
        authority: &CoordinatorAuthority,
        commit: AuthorityCommit,
    ) -> LocalAuthorityResult<DbResult<CoordinatorAuthority>> {
        use LocalAuthorityResult::{
            DurableFactEstablished as Established, DurableFactUnclassified as Unclassified,
        };
        let mut connection = match self.pool.acquire().await {
            Ok(connection) => connection,
            Err(error) => return Established(Err(error.into())),
        };
        let mut tx = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return Established(Err(error.into())),
        };
        let result = async {
            crate::product_conversation_rename::require_current_gc_tx(
                &mut tx,
                source_transcript_id,
            )
            .await?;
            sqlx::query(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(SETTING_KEY)
            .bind(authority.as_str())
            .execute(&mut *tx)
            .await?;
            read_on(&mut tx).await
        }
        .await;
        match result {
            Ok(authority) => match tx.commit().await {
                Ok(()) if matches!(commit, AuthorityCommit::Acknowledge) => {
                    Established(Ok(authority))
                }
                Ok(()) | Err(_) => Unclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }
}

#[cfg(test)]
mod tests;
