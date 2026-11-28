use std::collections::HashMap;

use phoenix_core::domain::close::{CloseAttemptId, CloseObligation, TranscriptConversationId};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::workflow::LocalAuthorityResult;
use tokio::sync::OwnedMutexGuard;

use super::close_retirement::GenericCloseIngressError;
use super::{CloseSettlementDelivery, RuntimeManager};
use crate::db::{Database, GenericCloseAdmission};

pub(super) enum GenericCloseAuthorityAdmission {
    Settle(GenericCloseSettlementOwner),
    Observe(Box<CloseObligation>),
}

pub(super) struct GenericCloseSettlementOwner {
    settlement: crate::db::GenericCloseSettlement,
    deliveries: HashMap<u64, CloseSettlementDelivery>,
}

pub(super) struct ActiveGenericCloseSettlement {
    execution: CloseAttemptOwner,
    owner: GenericCloseSettlementOwner,
}

impl ActiveGenericCloseSettlement {
    pub(super) fn into_parts(self) -> (CloseAttemptOwner, GenericCloseSettlementOwner) {
        (self.execution, self.owner)
    }
}

impl GenericCloseSettlementOwner {
    pub(super) fn settlement_and_deliveries(
        &mut self,
    ) -> (&crate::db::GenericCloseSettlement, &mut HashMap<u64, CloseSettlementDelivery>) {
        (&self.settlement, &mut self.deliveries)
    }

    pub(super) fn activate(
        self,
        execution: CloseAttemptOwner,
    ) -> Result<ActiveGenericCloseSettlement, String> {
        execution.require_exact(self.settlement.obligation().attempt_id())?;
        Ok(ActiveGenericCloseSettlement {
            execution,
            owner: self,
        })
    }
}

pub(crate) struct CloseAttemptOwner {
    attempt: CloseAttemptId,
    lease: Arc<OwnedMutexGuard<()>>,
}

impl CloseAttemptOwner {
    pub(super) fn retain_lease(&self) -> Arc<OwnedMutexGuard<()>> {
        Arc::clone(&self.lease)
    }

    pub(super) fn attempt(&self) -> &CloseAttemptId {
        &self.attempt
    }

    pub(super) fn require_exact(&self, attempt: &CloseAttemptId) -> Result<(), String> {
        if attempt != self.attempt() {
            return Err("Close continuation is not owned by this attempt".into());
        }
        Ok(())
    }
}

impl RuntimeManager {
    pub(super) async fn acquire_close_attempt(&self, attempt: CloseAttemptId) -> CloseAttemptOwner {
        let lease = self.close_retirement_execution.lock(attempt.as_str()).await;
        CloseAttemptOwner { attempt, lease: Arc::new(lease) }
    }

    pub(super) async fn reserve_native_initial_attempt(&self) -> Option<CloseAttemptOwner> {
        let attempt = CloseAttemptId::parse(uuid::Uuid::new_v4().to_string())
            .expect("UUID is an attempt ID");
        let lease = self.close_retirement_execution.try_lock(attempt.as_str()).await?;
        Some(CloseAttemptOwner { attempt, lease: Arc::new(lease) })
    }

    pub(super) async fn admit_generic_close(
        &self,
        owner: &CloseAttemptOwner,
        target: &ProductConversationId,
        latest: &TranscriptConversationId,
        stop_guard: Option<OwnedMutexGuard<()>>,
    ) -> Result<GenericCloseAuthorityAdmission, GenericCloseIngressError> {
        let result = if let Some(stop_guard) = stop_guard {
            let result = self
                .db()
                .confirm_generic_close_stop_work_classified(target, latest, owner.attempt())
                .await;
            let settlement = match result {
                LocalAuthorityResult::DurableFactEstablished(result) => {
                    result.map_err(GenericCloseIngressError::Admission)?
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    self.signal_fatal_local_authority("generic_close_stop_confirmation_unclassified");
                    return Err("generic Close stop-work transaction release is unclassified"
                        .to_string()
                        .into());
                }
            };
            drop(stop_guard);
            GenericCloseAdmission::InsertedSettling(settlement)
        } else {
            match self
                .db()
                .begin_generic_close_settlement_classified(target, latest, owner.attempt())
                .await
            {
                LocalAuthorityResult::DurableFactEstablished(result) => {
                    result.map_err(GenericCloseIngressError::Admission)?
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    self.signal_fatal_local_authority("generic_close_initial_admission_unclassified");
                    return Err("generic Close Initial admission transaction release is unclassified"
                        .to_string()
                        .into());
                }
            }
        };
        match result {
            GenericCloseAdmission::InsertedSettling(settlement) => {
                if let Err(error) = owner.require_exact(settlement.obligation().attempt_id()) {
                    self.signal_fatal_local_authority("generic_close_admission_owner_mismatch");
                    return Err(error.into());
                }
                Ok(GenericCloseAuthorityAdmission::Settle(GenericCloseSettlementOwner {
                    settlement,
                    deliveries: Default::default(),
                }))
            }
            GenericCloseAdmission::InsertedAwaitingStopWork(obligation)
            | GenericCloseAdmission::Existing(obligation) => {
                Ok(GenericCloseAuthorityAdmission::Observe(Box::new(obligation)))
            }
        }
    }

    pub(crate) async fn recover_interrupted_close_runs(
        &self,
        after_commit: impl FnMut(),
    ) -> Result<usize, Box<dyn std::error::Error>> {
        self.restore_close_admission_fences()
            .await
            .map_err(std::io::Error::other)?;
        // Startup classifies interrupted owners; it does not create a continuation.
        observe_running_close_runs(self.db(), after_commit).await
    }
}

pub(crate) async fn observe_running_close_runs(
    db: &Database,
    mut after_commit: impl FnMut(),
) -> Result<usize, Box<dyn std::error::Error>> {
    let running = db.list_running_close_runs().await?;
    let mut classified = 0;
    for run in running {
        db.classify_interrupted_close_run(&run).await?;
        classified += 1;
        after_commit();
    }
    Ok(classified)
}
