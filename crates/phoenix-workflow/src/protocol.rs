use std::collections::BTreeMap;

use crate::types::{
    AcceptanceStatus, DeliveryStatus, DivergenceSeverity, DrainCategoryEvidence, DrainProof,
    EffectRole, EffectStatus, ResolutionStatus, WorkflowProfile, WorkflowState, WorkflowStatus,
};

#[must_use]
pub fn exact_drain_categories() -> [&'static str; 8] {
    [
        "nonterminal_workflows",
        "active_or_unexpired_claims",
        "eligible_or_retry_effects",
        "uncompensated_effects",
        "unresolved_manual_resolutions",
        "pending_reducer_inbox",
        "owed_runtime_acceptances",
        "blocking_divergences",
    ]
}

#[must_use]
pub fn drain_proof<P: WorkflowProfile>(workflow: &WorkflowState<P>) -> DrainProof {
    let mut categories = BTreeMap::new();
    insert_drain_category(
        &mut categories,
        "nonterminal_workflows",
        nonterminal_workflows_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "active_or_unexpired_claims",
        active_claims_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "eligible_or_retry_effects",
        eligible_or_retry_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "uncompensated_effects",
        uncompensated_effects_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "unresolved_manual_resolutions",
        unresolved_manual_resolutions_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "pending_reducer_inbox",
        pending_reducer_inbox_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "owed_runtime_acceptances",
        owed_runtime_acceptances_evidence(workflow),
    );
    insert_drain_category(
        &mut categories,
        "blocking_divergences",
        blocking_divergences_evidence(workflow),
    );
    DrainProof {
        profile: workflow.binding.accepted_protocol().profile.clone(),
        protocol: workflow.binding.accepted_protocol().clone(),
        selector: workflow.binding.accepted_protocol().selector,
        authority: workflow.semantic_authority,
        categories,
    }
}

fn insert_drain_category(
    categories: &mut BTreeMap<&'static str, DrainCategoryEvidence>,
    key: &'static str,
    evidence: DrainCategoryEvidence,
) {
    categories.insert(key, evidence);
}

fn nonterminal_workflows_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    let terminal = matches!(
        workflow.status,
        WorkflowStatus::Cancelled | WorkflowStatus::Completed | WorkflowStatus::Failed
    );
    DrainCategoryEvidence {
        count: usize::from(!terminal),
        identities: if terminal {
            Vec::new()
        } else {
            vec![format!("workflow:{}", workflow.binding.workflow_id().0)]
        },
    }
}

fn active_claims_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .effects
            .values()
            .filter(|effect| effect.claim.is_some())
            .count(),
        identities: workflow
            .effects
            .values()
            .filter_map(|effect| {
                effect.claim.as_ref().map(|claim| {
                    format!(
                        "effect:{}:{}",
                        effect.declaration.effect_id.0, claim.claim_token
                    )
                })
            })
            .collect(),
    }
}

fn eligible_or_retry_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    effect_identities_by_status(workflow, |status| {
        matches!(status, EffectStatus::Eligible | EffectStatus::RetryWait)
    })
}

fn uncompensated_effects_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .effects
            .values()
            .filter(|effect| {
                effect.declaration.role == EffectRole::Compensation
                    && effect.status != EffectStatus::Receipted
            })
            .count(),
        identities: workflow
            .effects
            .values()
            .filter(|effect| {
                effect.declaration.role == EffectRole::Compensation
                    && effect.status != EffectStatus::Receipted
            })
            .map(|effect| format!("effect:{}", effect.declaration.effect_id.0))
            .collect(),
    }
}

fn unresolved_manual_resolutions_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .manual_resolutions
            .values()
            .filter(|resolution| resolution.status == ResolutionStatus::Required)
            .count(),
        identities: workflow
            .manual_resolutions
            .values()
            .filter(|resolution| resolution.status == ResolutionStatus::Required)
            .map(|resolution| format!("manual:{}", resolution.id.0))
            .collect(),
    }
}

fn pending_reducer_inbox_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .reducer_inbox
            .values()
            .filter(|event| event.delivery_status == DeliveryStatus::Pending)
            .count(),
        identities: workflow
            .reducer_inbox
            .values()
            .filter(|event| event.delivery_status == DeliveryStatus::Pending)
            .map(|event| format!("inbox:{}", event.id.0))
            .collect(),
    }
}

fn owed_runtime_acceptances_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .owed_acceptances
            .values()
            .filter(|owed| owed.status == AcceptanceStatus::Owed)
            .count(),
        identities: workflow
            .owed_acceptances
            .values()
            .filter(|owed| owed.status == AcceptanceStatus::Owed)
            .map(|owed| format!("owed:{}", owed.id.0))
            .collect(),
    }
}

fn blocking_divergences_evidence<P: WorkflowProfile>(
    workflow: &WorkflowState<P>,
) -> DrainCategoryEvidence {
    DrainCategoryEvidence {
        count: workflow
            .shadow_divergences
            .iter()
            .filter(|divergence| divergence.severity == DivergenceSeverity::Blocking)
            .count(),
        identities: workflow
            .shadow_divergences
            .iter()
            .filter(|divergence| divergence.severity == DivergenceSeverity::Blocking)
            .map(|divergence| divergence.evidence_identity.clone())
            .collect(),
    }
}

fn effect_identities_by_status<P, F>(
    workflow: &WorkflowState<P>,
    predicate: F,
) -> DrainCategoryEvidence
where
    P: WorkflowProfile,
    F: Fn(EffectStatus) -> bool,
{
    DrainCategoryEvidence {
        count: workflow
            .effects
            .values()
            .filter(|effect| predicate(effect.status))
            .count(),
        identities: workflow
            .effects
            .values()
            .filter(|effect| predicate(effect.status))
            .map(|effect| format!("effect:{}", effect.declaration.effect_id.0))
            .collect(),
    }
}
