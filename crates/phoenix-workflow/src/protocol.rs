use std::collections::BTreeMap;

use crate::types::{
    DeliveryStatus, DivergenceSeverity, DrainCategoryEvidence, DrainProof, EffectRole,
    EffectStatus, ExternalAcceptanceBinding, ExternalAcceptanceKey, ExternalAcceptanceOutcome,
    ExternalAcceptanceReceipt, NonEmptyExternalKey, ProtocolSelection, ResolutionStatus,
    ShadowDivergenceResolution, WorkflowId, WorkflowProfile, WorkflowState, WorkflowStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAcceptanceRegistry<H> {
    bindings: BTreeMap<ExternalAcceptanceKey, ExternalAcceptanceBinding<H>>,
}

impl<H: Clone + Eq> ExternalAcceptanceRegistry<H> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn accept(
        &mut self,
        selection: &ProtocolSelection,
        authority_scope: &str,
        idempotency_key: &str,
        intent_fingerprint: &str,
        workflow_id: WorkflowId,
        handle: H,
    ) -> ExternalAcceptanceOutcome<H> {
        let (Some(authority_scope), Some(idempotency_key)) = (
            NonEmptyExternalKey::new(authority_scope),
            NonEmptyExternalKey::new(idempotency_key),
        ) else {
            return ExternalAcceptanceOutcome::Unsupported;
        };
        let key = ExternalAcceptanceKey {
            profile: selection.profile.clone(),
            authority_scope,
            idempotency_key: idempotency_key.clone(),
        };
        if let Some(binding) = self.bindings.get(&key) {
            return if binding.intent_fingerprint == intent_fingerprint {
                ExternalAcceptanceOutcome::Replay(binding.receipt.clone())
            } else {
                ExternalAcceptanceOutcome::Conflict
            };
        }
        if !selection.accepting || !selection.external_acceptance_enabled {
            return ExternalAcceptanceOutcome::Unsupported;
        }
        let receipt = ExternalAcceptanceReceipt {
            idempotency_key,
            workflow_id,
            handle,
        };
        self.bindings.insert(
            key,
            ExternalAcceptanceBinding {
                intent_fingerprint: intent_fingerprint.to_owned(),
                receipt: receipt.clone(),
            },
        );
        ExternalAcceptanceOutcome::New(receipt)
    }
}

impl<H: Clone + Eq> Default for ExternalAcceptanceRegistry<H> {
    fn default() -> Self {
        Self::new()
    }
}

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
pub fn drain_proof<'a, P, I>(protocol: &ProtocolSelection, workflows: I) -> DrainProof
where
    P: WorkflowProfile + 'a,
    I: IntoIterator<Item = &'a WorkflowState<P>>,
{
    let mut categories = exact_drain_categories()
        .into_iter()
        .map(|category| {
            (
                category,
                DrainCategoryEvidence {
                    count: 0,
                    identities: Vec::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for workflow in workflows
        .into_iter()
        .filter(|workflow| same_protocol(workflow.binding.accepted_protocol(), protocol))
    {
        aggregate_workflow_evidence(&mut categories, workflow);
    }
    build_drain_proof(protocol, categories)
}

fn same_protocol(left: &ProtocolSelection, right: &ProtocolSelection) -> bool {
    left.profile == right.profile
        && left.authority == right.authority
        && left.selector == right.selector
}

fn aggregate_workflow_evidence<P: WorkflowProfile>(
    categories: &mut BTreeMap<&'static str, DrainCategoryEvidence>,
    workflow: &WorkflowState<P>,
) {
    if workflow.binding.execution_mode() == crate::ExecutionMode::Shadow {
        for category in exact_drain_categories() {
            let evidence = if category == "blocking_divergences" {
                blocking_divergences_evidence(workflow)
            } else {
                DrainCategoryEvidence {
                    count: 0,
                    identities: Vec::new(),
                }
            };
            insert_drain_category(categories, category, evidence);
        }
        return;
    }
    insert_drain_category(
        categories,
        "nonterminal_workflows",
        nonterminal_workflows_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "active_or_unexpired_claims",
        active_claims_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "eligible_or_retry_effects",
        eligible_or_retry_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "uncompensated_effects",
        uncompensated_effects_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "unresolved_manual_resolutions",
        unresolved_manual_resolutions_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "pending_reducer_inbox",
        pending_reducer_inbox_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "owed_runtime_acceptances",
        owed_runtime_acceptances_evidence(workflow),
    );
    insert_drain_category(
        categories,
        "blocking_divergences",
        blocking_divergences_evidence(workflow),
    );
}

fn build_drain_proof(
    protocol: &ProtocolSelection,
    categories: BTreeMap<&'static str, DrainCategoryEvidence>,
) -> DrainProof {
    let complete = exact_drain_categories()
        .into_iter()
        .all(|category| categories.contains_key(category))
        && categories.len() == exact_drain_categories().len();
    DrainProof {
        profile: protocol.profile.clone(),
        protocol: protocol.clone(),
        selector: protocol.selector,
        query_identity: "phoenix.workflow.drain",
        query_version: 1,
        authority: Some(protocol.authority),
        complete,
        categories,
    }
}

fn insert_drain_category(
    categories: &mut BTreeMap<&'static str, DrainCategoryEvidence>,
    key: &'static str,
    evidence: DrainCategoryEvidence,
) {
    let aggregate = categories.entry(key).or_insert(DrainCategoryEvidence {
        count: 0,
        identities: Vec::new(),
    });
    aggregate.count += evidence.count;
    aggregate.identities.extend(evidence.identities);
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
                    && !matches!(
                        effect.status,
                        EffectStatus::Receipted | EffectStatus::Invalidated
                    )
            })
            .count(),
        identities: workflow
            .effects
            .values()
            .filter(|effect| {
                effect.declaration.role == EffectRole::Compensation
                    && !matches!(
                        effect.status,
                        EffectStatus::Receipted | EffectStatus::Invalidated
                    )
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
            .filter(|owed| owed.disposition == crate::OwedAcceptanceDisposition::Owed)
            .count(),
        identities: workflow
            .owed_acceptances
            .values()
            .filter(|owed| owed.disposition == crate::OwedAcceptanceDisposition::Owed)
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
            .filter(|divergence| {
                divergence.severity == DivergenceSeverity::Blocking
                    && matches!(
                        divergence.resolution,
                        ShadowDivergenceResolution::Unresolved
                    )
            })
            .count(),
        identities: workflow
            .shadow_divergences
            .iter()
            .filter(|divergence| {
                divergence.severity == DivergenceSeverity::Blocking
                    && matches!(
                        divergence.resolution,
                        ShadowDivergenceResolution::Unresolved
                    )
            })
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
