use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

use crate::types::{
    BarrierId, BarrierMemberDecl, CodecRef, DeliveryDecl, EffectDecl, EffectId, EffectRole,
    Generation, ReceiptFamily, ScheduleId, SupportedCodecRegistry, TransitionPlan, WorkflowProfile,
    WorkflowStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    DuplicateEffectId(EffectId),
    EffectIdCollision(EffectId),
    DuplicateBarrierId(BarrierId),
    BarrierIdCollision(BarrierId),
    ScheduleIdCollision(ScheduleId),
    MissingCodec(&'static str),
    UnsupportedCodec(CodecRef),
    MissingEffectFamily(EffectId),
    MissingEffectKind(EffectId),
    CompensationOutsideCancellation(EffectId),
    NonCompensationInCancellation(EffectId),
    TerminalPlanDeclaresEffects(WorkflowStatus),
    EffectFamilyAmbiguityMismatch {
        family: &'static str,
    },
    UnknownEffectReference(EffectId),
    RequiredDependsOnOptional {
        effect_id: EffectId,
        optional_dependency: EffectId,
    },
    UnknownBarrierReference(BarrierId),
    UnknownInvalidationTarget(EffectId),
    InvalidatesReceiptedEffect(EffectId),
    EmptyDeliveryBatch,
    DeliverySourceCount {
        effect_id: Option<EffectId>,
        barrier_id: Option<BarrierId>,
    },
    RuntimeStartNotAllowed,
    DependencyCycle,
    DuplicateDependency {
        effect_id: EffectId,
        depends_on_effect_id: EffectId,
    },
    DuplicateBarrierMember {
        barrier_id: BarrierId,
        effect_id: EffectId,
    },
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
    #[error("workflow profile does not match the selected protocol profile")]
    ProfileProtocolMismatch,
    #[error("validated plan omitted barrier event for barrier {0:?}")]
    MissingValidatedBarrierEvent(BarrierId),
    #[error("reducer inbox item is missing or already consumed")]
    InvalidInbox,
    #[error("attempt count exceeds u32")]
    AttemptOrdinalOverflow,
    #[error("unsupported codec: {0:?}")]
    UnsupportedCodec(CodecRef),
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
    supported_codecs: &SupportedCodecRegistry,
) -> Result<(), PlanError> {
    validate_status_transition(current_status, plan.next_status)?;
    validate_plan_body(plan, barrier_events, supported_codecs)
}

pub(crate) fn validate_plan_body<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    barrier_events: &BTreeMap<BarrierId, P::BarrierEvent>,
    supported_codecs: &SupportedCodecRegistry,
) -> Result<(), PlanError> {
    validate_plan_codecs(plan, supported_codecs)?;
    if matches!(
        plan.next_status,
        WorkflowStatus::Cancelled
            | WorkflowStatus::Deleted
            | WorkflowStatus::Completed
            | WorkflowStatus::Failed
    ) && !plan.effects.is_empty()
    {
        return Err(PlanError::TerminalPlanDeclaresEffects(plan.next_status));
    }
    let effect_ids = collect_effect_ids(plan)?;
    let barrier_ids = collect_barrier_ids(plan, barrier_events)?;
    let mut schedule_ids = BTreeSet::new();
    let mut schedule_keys = BTreeSet::new();
    for schedule in &plan.schedules {
        if !schedule_ids.insert(schedule.schedule_id) {
            return Err(PlanError::ScheduleIdCollision(schedule.schedule_id));
        }
        if !schedule_keys.insert(schedule.key) {
            return Err(PlanError::ScheduleIdCollision(schedule.schedule_id));
        }
    }
    validate_deliveries(plan, &effect_ids, &barrier_ids)?;
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
            WorkflowStatus::Active | WorkflowStatus::Completed | WorkflowStatus::Failed
        ),
        WorkflowStatus::Cancelling => {
            matches!(
                next_status,
                WorkflowStatus::Cancelling
                    | WorkflowStatus::Cancelled
                    | WorkflowStatus::DeletionPending
            )
        }
        WorkflowStatus::ManualResolution => matches!(
            next_status,
            WorkflowStatus::ManualResolution
                | WorkflowStatus::Active
                | WorkflowStatus::Cancelling
                | WorkflowStatus::Cancelled
                | WorkflowStatus::DeletionPending
                | WorkflowStatus::Completed
                | WorkflowStatus::Failed
        ),
        WorkflowStatus::Cancelled | WorkflowStatus::Failed => {
            next_status == WorkflowStatus::DeletionPending
        }
        WorkflowStatus::Incompatible | WorkflowStatus::Deleted | WorkflowStatus::Completed => false,
        WorkflowStatus::DeletionPending => matches!(next_status, WorkflowStatus::Deleted),
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

fn validate_plan_codecs<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    supported_codecs: &SupportedCodecRegistry,
) -> Result<(), PlanError> {
    if plan.snapshot_codec.family.is_empty() {
        return Err(PlanError::MissingCodec("snapshot"));
    }
    if plan.event_codec.family.is_empty() {
        return Err(PlanError::MissingCodec("event"));
    }
    for effect in &plan.effects {
        if effect.family.is_empty() {
            return Err(PlanError::MissingEffectFamily(effect.effect_id));
        }
        if effect.kind.is_empty() {
            return Err(PlanError::MissingEffectKind(effect.effect_id));
        }
        if effect.codec.family.is_empty() {
            return Err(PlanError::MissingCodec("effect"));
        }
    }
    for barrier in &plan.barriers {
        if barrier.reducer_event_codec.family.is_empty() {
            return Err(PlanError::MissingCodec("barrier"));
        }
    }
    for codec in std::iter::once(&plan.snapshot_codec)
        .chain(std::iter::once(&plan.event_codec))
        .chain(plan.effects.iter().map(|effect| &effect.codec))
        .chain(
            plan.barriers
                .iter()
                .map(|barrier| &barrier.reducer_event_codec),
        )
        .chain(plan.deliveries.iter().map(|delivery| &delivery.event_codec))
    {
        if !supported_codecs.supports(codec) {
            return Err(PlanError::UnsupportedCodec(codec.clone()));
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

fn validate_deliveries<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    effect_ids: &BTreeSet<EffectId>,
    barrier_ids: &BTreeSet<BarrierId>,
) -> Result<(), PlanError> {
    for delivery in &plan.deliveries {
        validate_delivery_source(delivery, effect_ids, barrier_ids)?;
        if delivery.requires_runtime_acceptance() && !P::runtime_start_allowed(&plan.snapshot) {
            return Err(PlanError::RuntimeStartNotAllowed);
        }
    }
    Ok(())
}

fn validate_delivery_source<P: WorkflowProfile>(
    delivery: &DeliveryDecl<P>,
    effect_ids: &BTreeSet<EffectId>,
    barrier_ids: &BTreeSet<BarrierId>,
) -> Result<(), PlanError> {
    let source_count =
        usize::from(delivery.effect_id.is_some()) + usize::from(delivery.barrier_id.is_some());
    if source_count != 1 {
        return Err(PlanError::DeliverySourceCount {
            effect_id: delivery.effect_id,
            barrier_id: delivery.barrier_id,
        });
    }
    if let Some(effect_id) = delivery.effect_id {
        if !effect_ids.contains(&effect_id) {
            return Err(PlanError::UnknownEffectReference(effect_id));
        }
    }
    if let Some(barrier_id) = delivery.barrier_id {
        if !barrier_ids.contains(&barrier_id) {
            return Err(PlanError::UnknownBarrierReference(barrier_id));
        }
    }
    Ok(())
}

fn validate_dependencies<P: WorkflowProfile>(
    plan: &TransitionPlan<P>,
    effect_ids: &BTreeSet<EffectId>,
) -> Result<(), PlanError> {
    let mut dependency_pairs = BTreeSet::new();
    for dependency in &plan.dependencies {
        let pair = (dependency.effect_id, dependency.depends_on_effect_id);
        if !dependency_pairs.insert(pair) {
            return Err(PlanError::DuplicateDependency {
                effect_id: dependency.effect_id,
                depends_on_effect_id: dependency.depends_on_effect_id,
            });
        }
        if !effect_ids.contains(&dependency.effect_id) {
            return Err(PlanError::UnknownEffectReference(dependency.effect_id));
        }
        if !effect_ids.contains(&dependency.depends_on_effect_id) {
            return Err(PlanError::UnknownEffectReference(
                dependency.depends_on_effect_id,
            ));
        }
        let effect = plan
            .effects
            .iter()
            .find(|effect| effect.effect_id == dependency.effect_id)
            .expect("effect ids were validated");
        let prerequisite = plan
            .effects
            .iter()
            .find(|effect| effect.effect_id == dependency.depends_on_effect_id)
            .expect("dependency ids were validated");
        if effect.role == EffectRole::Required && prerequisite.role == EffectRole::Optional {
            return Err(PlanError::RequiredDependsOnOptional {
                effect_id: dependency.effect_id,
                optional_dependency: dependency.depends_on_effect_id,
            });
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
        if members_by_barrier
            .get(&member.barrier_id)
            .is_some_and(|members| {
                members
                    .iter()
                    .any(|existing| existing.effect_id == member.effect_id)
            })
        {
            return Err(PlanError::DuplicateBarrierMember {
                barrier_id: member.barrier_id,
                effect_id: member.effect_id,
            });
        }
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
