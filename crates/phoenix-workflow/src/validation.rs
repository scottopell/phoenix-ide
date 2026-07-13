use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

use crate::types::{
    BarrierId, BarrierMemberDecl, EffectDecl, EffectId, EffectRole, Generation, ReceiptFamily,
    TransitionPlan, WorkflowProfile, WorkflowStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    DuplicateEffectId(EffectId),
    EffectIdCollision(EffectId),
    DuplicateBarrierId(BarrierId),
    BarrierIdCollision(BarrierId),
    MissingCodec(&'static str),
    UnknownEffectReference(EffectId),
    UnknownBarrierReference(BarrierId),
    UnknownInvalidationTarget(EffectId),
    InvalidatesReceiptedEffect(EffectId),
    DependencyCycle,
    BarrierHasNoMembers(BarrierId),
    BarrierIncludesNonRequiredEffect {
        barrier_id: BarrierId,
        effect_id: EffectId,
    },
    BarrierReceiptFamilyMismatch {
        barrier_id: BarrierId,
        effect_id: EffectId,
    },
    EffectGenerationMismatch {
        effect_id: EffectId,
        expected: Generation,
        actual: Generation,
    },
    InvalidatesManualResolutionEffect(EffectId),
    InvalidStatusTransition {
        current: WorkflowStatus,
        next: WorkflowStatus,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EngineError {
    #[error("plan validation failed: {0:?}")]
    InvalidPlan(PlanError),
    #[error("protocol selection is not accepting new workflows")]
    ProtocolNotAccepting,
    #[error("workflow profile does not match the selected protocol profile")]
    ProfileProtocolMismatch,
    #[error("workflow binding is shadow-only and cannot execute")]
    ShadowCannotExecute,
    #[error("validated plan omitted barrier event for barrier {0:?}")]
    MissingValidatedBarrierEvent(BarrierId),
    #[error("reducer inbox item is missing or already consumed")]
    InvalidInbox,
    #[error("attempt count exceeds u32")]
    AttemptOrdinalOverflow,
}

#[must_use]
pub fn declared_receipt_family<I>(effect: &EffectDecl<I>) -> ReceiptFamily {
    match effect.role {
        EffectRole::Compensation => ReceiptFamily::CompensationEffect,
        EffectRole::Required | EffectRole::Optional => ReceiptFamily::CurrentGenerationEffect,
    }
}

/// Validates that a reducer transition plan is self-consistent and references only declared
/// effects and barriers.
///
/// # Errors
/// Returns [`PlanError`] when codecs are missing, references are dangling, barriers violate
/// membership rules, or dependencies contain a cycle.
pub fn validate_plan<P: WorkflowProfile>(
    current_status: WorkflowStatus,
    plan: &TransitionPlan<P>,
    barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
) -> Result<(), PlanError> {
    validate_status_transition(current_status, plan.next_status)?;
    validate_plan_codecs(plan)?;
    let effect_ids = collect_effect_ids(plan)?;
    let barrier_ids = collect_barrier_ids(plan, barrier_events)?;
    validate_dependencies(plan, &effect_ids)?;
    let members_by_barrier = collect_barrier_members(plan, &effect_ids, &barrier_ids)?;
    validate_barrier_members(plan, &members_by_barrier)?;
    validate_dependency_cycles(plan)?;
    Ok(())
}

pub(crate) fn validate_status_transition(
    current_status: WorkflowStatus,
    next_status: WorkflowStatus,
) -> Result<(), PlanError> {
    let valid = match current_status {
        WorkflowStatus::Active => matches!(
            next_status,
            WorkflowStatus::Active
                | WorkflowStatus::Cancelling
                | WorkflowStatus::Cancelled
                | WorkflowStatus::DeletionPending
                | WorkflowStatus::Completed
                | WorkflowStatus::Failed
        ),
        WorkflowStatus::Cancelling => {
            matches!(
                next_status,
                WorkflowStatus::Cancelling | WorkflowStatus::Cancelled
            )
        }
        WorkflowStatus::Cancelled
        | WorkflowStatus::DeletionPending
        | WorkflowStatus::Completed
        | WorkflowStatus::Failed => next_status == current_status,
    };
    if valid {
        Ok(())
    } else {
        Err(PlanError::InvalidStatusTransition {
            current: current_status,
            next: next_status,
        })
    }
}

fn validate_plan_codecs<P: WorkflowProfile>(plan: &TransitionPlan<P>) -> Result<(), PlanError> {
    if plan.snapshot_codec.family.is_empty() {
        return Err(PlanError::MissingCodec("snapshot"));
    }
    if plan.event_codec.family.is_empty() {
        return Err(PlanError::MissingCodec("event"));
    }
    for effect in &plan.effects {
        if effect.codec.family.is_empty() {
            return Err(PlanError::MissingCodec("effect"));
        }
    }
    for barrier in &plan.barriers {
        if barrier.reducer_event_codec.family.is_empty() {
            return Err(PlanError::MissingCodec("barrier"));
        }
    }
    if let Some(owed_acceptances) = &plan.owed_acceptances {
        for owed in owed_acceptances {
            if owed.event_codec.family.is_empty() {
                return Err(PlanError::MissingCodec("owed_acceptance"));
            }
        }
    }
    Ok(())
}

fn collect_effect_ids<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
) -> Result<BTreeSet<EffectId>, PlanError> {
    let mut effect_ids = BTreeSet::new();
    for effect in &plan.effects {
        if !effect_ids.insert(effect.effect_id) {
            return Err(PlanError::DuplicateEffectId(effect.effect_id));
        }
    }
    Ok(effect_ids)
}

fn collect_barrier_ids<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
) -> Result<BTreeSet<BarrierId>, PlanError> {
    let mut barrier_ids = BTreeSet::new();
    for barrier in &plan.barriers {
        if !barrier_ids.insert(barrier.barrier_id) {
            return Err(PlanError::DuplicateBarrierId(barrier.barrier_id));
        }
        if !barrier_events.contains_key(&barrier.barrier_id) {
            return Err(PlanError::UnknownBarrierReference(barrier.barrier_id));
        }
    }
    Ok(barrier_ids)
}

fn validate_dependencies<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    effect_ids: &BTreeSet<EffectId>,
) -> Result<(), PlanError> {
    for dependency in &plan.dependencies {
        if !effect_ids.contains(&dependency.effect_id) {
            return Err(PlanError::UnknownEffectReference(dependency.effect_id));
        }
        if !effect_ids.contains(&dependency.depends_on_effect_id) {
            return Err(PlanError::UnknownEffectReference(
                dependency.depends_on_effect_id,
            ));
        }
    }
    Ok(())
}

fn collect_barrier_members<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    effect_ids: &BTreeSet<EffectId>,
    barrier_ids: &BTreeSet<BarrierId>,
) -> Result<BTreeMap<BarrierId, Vec<BarrierMemberDecl>>, PlanError> {
    let mut members_by_barrier: BTreeMap<BarrierId, Vec<BarrierMemberDecl>> = BTreeMap::new();
    for member in &plan.barrier_members {
        if !barrier_ids.contains(&member.barrier_id) {
            return Err(PlanError::UnknownBarrierReference(member.barrier_id));
        }
        if !effect_ids.contains(&member.effect_id) {
            return Err(PlanError::UnknownEffectReference(member.effect_id));
        }
        members_by_barrier
            .entry(member.barrier_id)
            .or_default()
            .push(*member);
    }
    Ok(members_by_barrier)
}

fn validate_barrier_members<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    members_by_barrier: &BTreeMap<BarrierId, Vec<BarrierMemberDecl>>,
) -> Result<(), PlanError> {
    let effect_by_id: BTreeMap<EffectId, &EffectDecl<P::Intent>> = plan
        .effects
        .iter()
        .map(|effect| (effect.effect_id, effect))
        .collect();

    for barrier in &plan.barriers {
        let Some(members) = members_by_barrier.get(&barrier.barrier_id) else {
            return Err(PlanError::BarrierHasNoMembers(barrier.barrier_id));
        };
        validate_single_barrier(barrier.barrier_id, members, &effect_by_id)?;
    }
    Ok(())
}

fn validate_single_barrier<I>(
    barrier_id: BarrierId,
    members: &[BarrierMemberDecl],
    effect_by_id: &BTreeMap<EffectId, &EffectDecl<I>>,
) -> Result<(), PlanError> {
    let mut compensation_only = true;
    for member in members {
        let effect = effect_by_id[&member.effect_id];
        compensation_only &= effect.role == EffectRole::Compensation;
        let expected = declared_receipt_family(effect);
        if member.receipt_family != expected {
            return Err(PlanError::BarrierReceiptFamilyMismatch {
                barrier_id,
                effect_id: member.effect_id,
            });
        }
    }
    if !compensation_only {
        for member in members {
            let effect = effect_by_id[&member.effect_id];
            if effect.role != EffectRole::Required {
                return Err(PlanError::BarrierIncludesNonRequiredEffect {
                    barrier_id,
                    effect_id: member.effect_id,
                });
            }
        }
    }
    Ok(())
}

fn validate_dependency_cycles<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
) -> Result<(), PlanError> {
    let mut indegree: BTreeMap<EffectId, usize> = plan
        .effects
        .iter()
        .map(|effect| (effect.effect_id, 0))
        .collect();
    let mut outgoing: BTreeMap<EffectId, Vec<EffectId>> = BTreeMap::new();
    for dependency in &plan.dependencies {
        let Some(count) = indegree.get_mut(&dependency.effect_id) else {
            return Err(PlanError::UnknownEffectReference(dependency.effect_id));
        };
        *count += 1;
        outgoing
            .entry(dependency.depends_on_effect_id)
            .or_default()
            .push(dependency.effect_id);
    }
    let mut queue: VecDeque<EffectId> = indegree
        .iter()
        .filter_map(|(effect_id, count)| (*count == 0).then_some(*effect_id))
        .collect();
    let mut visited = 0usize;
    while let Some(effect_id) = queue.pop_front() {
        visited += 1;
        if let Some(children) = outgoing.get(&effect_id) {
            for child in children {
                let Some(count) = indegree.get_mut(child) else {
                    return Err(PlanError::UnknownEffectReference(*child));
                };
                *count -= 1;
                if *count == 0 {
                    queue.push_back(*child);
                }
            }
        }
    }
    if visited != indegree.len() {
        return Err(PlanError::DependencyCycle);
    }
    Ok(())
}
