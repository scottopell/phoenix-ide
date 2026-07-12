use std::collections::BTreeMap;

use crate::{
    AttemptId, BarrierId, CancellationRequest, ClaimAuthority, EffectId, LeaseExpiry,
    ReducerDecision, Timestamp, WorkflowProfile, WorkflowState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimOp<P: WorkflowProfile> {
    Commit {
        decision: ReducerDecision<P>,
        barrier_events: BTreeMap<BarrierId, P::BarrierEvent>,
    },
    Cancel {
        request: CancellationRequest<P>,
        barrier_events: BTreeMap<BarrierId, P::BarrierEvent>,
    },
    Claim {
        effect_id: EffectId,
        worker_id: &'static str,
        lease_until: LeaseExpiry,
    },
    Renew {
        authority: ClaimAuthority,
        lease_until: LeaseExpiry,
    },
    Observe {
        authority: ClaimAuthority,
        attempt_id: AttemptId,
        observation: P::Observation,
    },
    Retry {
        authority: ClaimAuthority,
        retry_at: Timestamp,
    },
    AdvanceTime(Timestamp),
    Restart,
    CrashWorker {
        worker_id: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulator<P: WorkflowProfile> {
    pub now: Timestamp,
    pub workflow: WorkflowState<P>,
}

impl<P: WorkflowProfile> Simulator<P> {
    #[must_use]
    pub fn new(workflow: WorkflowState<P>) -> Self {
        Self {
            now: Timestamp(0),
            workflow,
        }
    }

    pub fn apply(&mut self, op: SimOp<P>) {
        match op {
            SimOp::Commit {
                decision,
                barrier_events,
            } => {
                let _ = self.workflow.commit_transition(&decision, &barrier_events);
            }
            SimOp::Cancel {
                request,
                barrier_events,
            } => {
                let _ = self
                    .workflow
                    .cancel_with_compensation(&request, &barrier_events);
            }
            SimOp::Claim {
                effect_id,
                worker_id,
                lease_until,
            } => {
                let _ = self
                    .workflow
                    .claim_effect(effect_id, worker_id, self.now, lease_until);
            }
            SimOp::Renew {
                authority,
                lease_until,
            } => {
                let _ = self.workflow.renew_claim(&authority, self.now, lease_until);
            }
            SimOp::Observe {
                authority,
                attempt_id,
                observation,
            } => {
                let _ = self.workflow.record_observation(
                    &authority,
                    self.now,
                    attempt_id,
                    observation,
                    true,
                );
            }
            SimOp::Retry {
                authority,
                retry_at,
            } => {
                let _ = self.workflow.schedule_retry(&authority, self.now, retry_at);
            }
            SimOp::AdvanceTime(now) => {
                self.now = now;
                self.workflow.refresh_eligibility(now);
            }
            SimOp::Restart => {
                self.workflow.crashed_workers.clear();
                self.workflow.refresh_eligibility(self.now);
            }
            SimOp::CrashWorker { worker_id } => {
                self.workflow.crashed_workers.insert(worker_id);
            }
        }
    }
}
