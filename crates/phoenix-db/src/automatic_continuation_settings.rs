use phoenix_core::domain::product_conversation::{
    AutoContinueOnContextExhaustion, ProductConversationId,
};
use sqlx::{Connection, SqliteConnection};

use crate::{workflow::LocalAuthorityResult, Database, DbError, DbResult};

#[derive(Debug, Clone, Copy)]
pub enum AutomaticContinuationTarget<'a> {
    Ordinary(&'a ProductConversationId),
    Coordinator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAutomaticContinuationAggregate {
    Ordinary(ProductConversationId),
    Coordinator(ProductConversationId),
}

impl ResolvedAutomaticContinuationAggregate {
    #[must_use]
    pub fn product_conversation_id(&self) -> &ProductConversationId {
        match self {
            Self::Ordinary(id) | Self::Coordinator(id) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomaticContinuationSettings {
    pub aggregate: ResolvedAutomaticContinuationAggregate,
    pub preference: AutoContinueOnContextExhaustion,
}

pub(crate) async fn read_preference_on(
    connection: &mut SqliteConnection,
    id: &ProductConversationId,
) -> DbResult<AutoContinueOnContextExhaustion> {
    let value: Option<i64> = sqlx::query_scalar(
        "SELECT auto_continue_on_context_exhaustion FROM product_conversations WHERE id = ?1",
    )
    .bind(id.as_str())
    .fetch_optional(connection)
    .await?;
    value
        .map(|value| AutoContinueOnContextExhaustion::from(value != 0))
        .ok_or_else(|| DbError::ProductConversationUnavailable(id.clone()))
}

pub(crate) async fn write_preference_on(
    connection: &mut SqliteConnection,
    id: &ProductConversationId,
    preference: AutoContinueOnContextExhaustion,
) -> DbResult<()> {
    let updated = sqlx::query(
        "UPDATE product_conversations SET auto_continue_on_context_exhaustion = ?1 WHERE id = ?2",
    )
    .bind(i64::from(bool::from(preference)))
    .bind(id.as_str())
    .execute(connection)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(DbError::ProductConversationUnavailable(id.clone()));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SettingsCommit {
    Acknowledge,
    #[cfg(any(test, feature = "test-support"))]
    LoseAcknowledgement,
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) struct SettingsCommitTestPause {
    committed: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl Database {
    /// Arms one native settings write to lose its commit acknowledgement, pausing
    /// before returning the unclassified outcome until the release sender is used.
    /// The control is shared only by clones of this DB.
    ///
    /// # Panics
    /// Panics if another native settings acknowledgement pause is already armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_native_settings_commit_ack_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (committed, observed) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let mut pause = self.native_settings_commit_test_pause.lock().unwrap();
        assert!(
            pause.is_none(),
            "native settings commit pause already armed"
        );
        *pause = Some(SettingsCommitTestPause {
            committed,
            release: released,
        });
        (observed, release)
    }

    /// Read a peer's preference under its current send enrollment.
    ///
    /// # Errors
    /// Returns an error for inactive authority, unavailable target or storage failure.
    pub async fn read_federated_automatic_continuation_settings(
        &self,
        authority: &crate::FederationSendAuthority,
        target: AutomaticContinuationTarget<'_>,
    ) -> DbResult<AutomaticContinuationSettings> {
        let mut tx = self.pool.begin().await?;
        crate::federation_enrollment::require_federation_send_authority_tx(&mut tx, authority)
            .await?;
        let aggregate = Self::resolve_settings_target(&mut tx, target).await?;
        let preference = read_preference_on(&mut tx, aggregate.product_conversation_id()).await?;
        tx.commit().await?;
        Ok(AutomaticContinuationSettings {
            aggregate,
            preference,
        })
    }

    /// Set a peer's preference under its current send enrollment.
    ///
    /// # Errors
    /// Returns an established error after acknowledged rollback; uncertain commit or rollback
    /// acknowledgement remains unclassified.
    pub async fn set_federated_automatic_continuation_settings(
        &self,
        authority: &crate::FederationSendAuthority,
        target: AutomaticContinuationTarget<'_>,
        preference: AutoContinueOnContextExhaustion,
    ) -> LocalAuthorityResult<DbResult<AutomaticContinuationSettings>> {
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
            crate::federation_enrollment::require_federation_send_authority_tx(&mut tx, authority)
                .await?;
            let aggregate = Self::resolve_settings_target(&mut tx, target).await?;
            write_preference_on(&mut tx, aggregate.product_conversation_id(), preference).await?;
            let preference =
                read_preference_on(&mut tx, aggregate.product_conversation_id()).await?;
            Ok(AutomaticContinuationSettings {
                aggregate,
                preference,
            })
        }
        .await;
        match result {
            Ok(settings) => match tx.commit().await {
                Ok(()) => Established(Ok(settings)),
                Err(_) => Unclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }

    /// Reads the current preference for a verified transcript and target.
    ///
    /// # Errors
    /// Returns an error when the source transcript is not the current Coordinator,
    /// the ordinary target is not the exact published aggregate, or storage fails.
    pub async fn read_native_automatic_continuation_settings(
        &self,
        source_transcript_id: &str,
        target: AutomaticContinuationTarget<'_>,
    ) -> DbResult<AutomaticContinuationSettings> {
        let mut tx = self.pool.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, source_transcript_id)
            .await?;
        let aggregate = Self::resolve_settings_target(&mut tx, target).await?;
        let preference = read_preference_on(&mut tx, aggregate.product_conversation_id()).await?;
        tx.commit().await?;
        Ok(AutomaticContinuationSettings {
            aggregate,
            preference,
        })
    }

    /// Sets the preference for a verified transcript and target.
    ///
    /// # Errors
    /// Returns an established error after acknowledged rollback. A commit or
    /// rollback whose acknowledgement is lost is reported as unclassified.
    ///
    /// # Panics
    /// In tests or with `test-support`, panics if the commit-pause mutex is
    /// poisoned, a paused write does not return unclassified, or its release
    /// controller is dropped before releasing it.
    pub async fn set_native_automatic_continuation_settings(
        &self,
        source_transcript_id: &str,
        target: AutomaticContinuationTarget<'_>,
        preference: AutoContinueOnContextExhaustion,
    ) -> LocalAuthorityResult<DbResult<AutomaticContinuationSettings>> {
        #[cfg(any(test, feature = "test-support"))]
        {
            let pause = self
                .native_settings_commit_test_pause
                .lock()
                .unwrap()
                .take();
            if let Some(pause) = pause {
                let result = self
                    .set_native_automatic_continuation_settings_with_commit(
                        source_transcript_id,
                        target,
                        preference,
                        SettingsCommit::LoseAcknowledgement,
                    )
                    .await;
                assert!(matches!(
                    result,
                    LocalAuthorityResult::DurableFactUnclassified
                ));
                let _ = pause.committed.send(());
                pause
                    .release
                    .await
                    .expect("native settings commit pause controller dropped");
                return result;
            }
        }
        self.set_native_automatic_continuation_settings_with_commit(
            source_transcript_id,
            target,
            preference,
            SettingsCommit::Acknowledge,
        )
        .await
    }

    async fn set_native_automatic_continuation_settings_with_commit(
        &self,
        source_transcript_id: &str,
        target: AutomaticContinuationTarget<'_>,
        preference: AutoContinueOnContextExhaustion,
        commit: SettingsCommit,
    ) -> LocalAuthorityResult<DbResult<AutomaticContinuationSettings>> {
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
            let aggregate = Self::resolve_settings_target(&mut tx, target).await?;
            write_preference_on(&mut tx, aggregate.product_conversation_id(), preference).await?;
            let preference =
                read_preference_on(&mut tx, aggregate.product_conversation_id()).await?;
            Ok(AutomaticContinuationSettings {
                aggregate,
                preference,
            })
        }
        .await;
        match result {
            Ok(settings) => {
                let committed = tx.commit().await;
                match committed {
                    Ok(()) if matches!(commit, SettingsCommit::Acknowledge) => {
                        Established(Ok(settings))
                    }
                    Ok(()) | Err(_) => Unclassified,
                }
            }
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }

    async fn resolve_settings_target(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        target: AutomaticContinuationTarget<'_>,
    ) -> DbResult<ResolvedAutomaticContinuationAggregate> {
        match target {
            AutomaticContinuationTarget::Ordinary(id) => {
                let resolved =
                    Self::resolve_ordinary_product_conversation_on(tx, id.as_str()).await?;
                if resolved.product_conversation_id != *id {
                    return Err(DbError::ContinuationPrecondition(
                        "target did not resolve to the exact ordinary product conversation".into(),
                    ));
                }
                Ok(ResolvedAutomaticContinuationAggregate::Ordinary(id.clone()))
            }
            AutomaticContinuationTarget::Coordinator => {
                let id: Option<String> = sqlx::query_scalar(
                    "SELECT product_conversation_id FROM conversations WHERE runtime_role = 'coordinator' AND coordinator_head = 1",
                )
                .fetch_optional(&mut **tx)
                .await?;
                let id = id.ok_or(DbError::CurrentCoordinatorRequired)?;
                Ok(ResolvedAutomaticContinuationAggregate::Coordinator(
                    ProductConversationId::parse(id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                ))
            }
        }
    }
}

#[cfg(test)]
#[path = "automatic_continuation_settings_tests.rs"]
mod tests;
