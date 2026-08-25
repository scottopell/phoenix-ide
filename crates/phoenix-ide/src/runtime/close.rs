use std::collections::BTreeSet;
use std::sync::RwLock;

use phoenix_core::domain::close::{
    CloseAttemptId, CloseObligation, ClosePhase, ProductConversationId,
};
use phoenix_db::Database;

/// Runtime-owned recovery record for one persisted aggregate admission fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveredCloseFence {
    attempt_id: CloseAttemptId,
    product_conversation_id: ProductConversationId,
    phase: ClosePhase,
}

impl From<CloseObligation> for RecoveredCloseFence {
    fn from(obligation: CloseObligation) -> Self {
        Self {
            attempt_id: obligation.attempt_id().clone(),
            product_conversation_id: obligation.product_conversation_id().clone(),
            phase: obligation.phase(),
        }
    }
}

/// Runtime owner for persisted aggregate Close admission fences.
#[derive(Default)]
pub(crate) struct CloseAdmissionCoordinator {
    recovered_fences: RwLock<BTreeSet<RecoveredCloseFence>>,
}

impl Ord for RecoveredCloseFence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.attempt_id.cmp(&other.attempt_id)
    }
}

impl PartialOrd for RecoveredCloseFence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl CloseAdmissionCoordinator {
    pub(crate) async fn recover(&self, db: &Database) -> Result<(), String> {
        let obligations = db
            .list_pending_close_obligations()
            .await
            .map_err(|error| error.to_string())?;
        let mut recovered_fences = self
            .recovered_fences
            .write()
            .map_err(|_| "Close admission coordinator lock poisoned".to_string())?;
        recovered_fences.clear();
        recovered_fences.extend(obligations.into_iter().map(RecoveredCloseFence::from));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn recovered_fences(&self) -> Vec<RecoveredCloseFence> {
        self.recovered_fences
            .read()
            .expect("Close admission coordinator lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::CloseAdmissionCoordinator;
    use phoenix_core::domain::close::{CloseAttemptId, ClosePhase};
    use phoenix_db::Database;

    #[tokio::test]
    async fn recovery_orders_pending_fences_by_attempt_id() {
        let db = Database::open_in_memory().await.unwrap();
        for (conversation_id, attempt_id) in
            [("recovery-z", "z-attempt"), ("recovery-a", "a-attempt")]
        {
            db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
                .await
                .unwrap();
            let product_conversation_id = db
                .get_conversation(conversation_id)
                .await
                .unwrap()
                .product_conversation_id;
            db.begin_close_foundation(&product_conversation_id, attempt_id)
                .await
                .unwrap();
        }

        let coordinator = CloseAdmissionCoordinator::default();
        coordinator.recover(&db).await.unwrap();
        let attempts: Vec<_> = coordinator
            .recovered_fences()
            .into_iter()
            .map(|fence| fence.attempt_id)
            .collect();
        assert_eq!(
            attempts,
            vec![
                CloseAttemptId::parse("a-attempt").unwrap(),
                CloseAttemptId::parse("z-attempt").unwrap(),
            ]
        );
    }

    #[tokio::test]
    async fn recovery_reconstitutes_exact_pending_product_fences() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("close-recovery", "Close recovery", "/tmp", true, None, None)
            .await
            .unwrap();
        let product_conversation_id = db
            .get_conversation("close-recovery")
            .await
            .unwrap()
            .product_conversation_id;
        db.begin_close_foundation(&product_conversation_id, "recover-close-fence")
            .await
            .unwrap();

        let coordinator = CloseAdmissionCoordinator::default();
        coordinator.recover(&db).await.unwrap();
        assert_eq!(
            coordinator.recovered_fences(),
            vec![super::RecoveredCloseFence {
                attempt_id: CloseAttemptId::parse("recover-close-fence").unwrap(),
                product_conversation_id,
                phase: ClosePhase::AwaitingBlockerResolution,
            }]
        );
    }
}
