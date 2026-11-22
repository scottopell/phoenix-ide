//! Inert, process-epoch evidence associated with the original Close permits.
//! This is not authority to retire a worktree or to confirm a loss.

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptId, CloseAttemptScope, CloseInspection,
    CloseInspectionLoss, CloseLossItem, ClosePhase, CloseRetirementSnapshot, WorktreeIdentity,
};
use phoenix_core::work_scope::WorkScopeId;
use phoenix_db::{CaptureCloseRetirementInventoryRequest, Database};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runtime::close_retirement) struct CaptureEvidenceBinding {
    scope: WorkScopeId,
    aggregate: CloseRetirementSnapshot,
    worktree: Option<WorktreeIdentity>,
    content: Option<CloseRetirementSnapshot>,
    losses: Vec<CloseLossItem>,
    deadline: tokio::time::Instant,
}

#[derive(Debug)]
pub(in crate::runtime::close_retirement) struct OriginalWorktreeCapture<'a> {
    pub(in crate::runtime::close_retirement) identity: &'a WorktreeIdentity,
    pub(in crate::runtime::close_retirement) content: &'a CloseRetirementSnapshot,
    pub(in crate::runtime::close_retirement) losses: &'a [CloseLossItem],
    pub(in crate::runtime::close_retirement) deadline: tokio::time::Instant,
}

impl CaptureEvidenceBinding {
    pub(in crate::runtime::close_retirement) fn select_worktree(
        &self,
        scope: &WorkScopeId,
        aggregate: &CloseRetirementSnapshot,
        identity: &WorktreeIdentity,
    ) -> Result<OriginalWorktreeCapture<'_>, String> {
        if &self.scope != scope || &self.aggregate != aggregate {
            return Err("original worktree capture scope or aggregate differs".into());
        }
        let original = self
            .worktree
            .as_ref()
            .ok_or("original capture has no worktree")?;
        if original != identity {
            return Err("original worktree capture full identity differs".into());
        }
        let content = self
            .content
            .as_ref()
            .ok_or("original worktree capture has no content inspection")?;
        Ok(OriginalWorktreeCapture {
            identity: original,
            content,
            losses: &self.losses,
            deadline: self.deadline,
        })
    }

    pub(in crate::runtime::close_retirement) fn snapshot(&self) -> &CloseRetirementSnapshot {
        &self.aggregate
    }

    pub(in crate::runtime::close_retirement) fn verifies_request(
        &self,
        snapshot: &CloseRetirementSnapshot,
    ) -> bool {
        &self.aggregate == snapshot
    }

    pub(in crate::runtime::close_retirement) fn verifies_scope(
        &self,
        scope: &WorkScopeId,
        snapshot: &CloseRetirementSnapshot,
    ) -> bool {
        &self.scope == scope && &self.aggregate == snapshot
    }

    pub(in crate::runtime::close_retirement) fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
}

pub(in crate::runtime::close_retirement) struct ScopeCaptureEvidence {
    scope: WorkScopeId,
    aggregate: CloseRetirementSnapshot,
    worktree: Option<WorktreeIdentity>,
    content: Option<CloseRetirementSnapshot>,
    losses: Vec<CloseLossItem>,
}

impl ScopeCaptureEvidence {
    pub(in crate::runtime::close_retirement) fn scope(&self) -> &WorkScopeId {
        &self.scope
    }

    pub(in crate::runtime::close_retirement) fn bind(
        &self,
        deadline: tokio::time::Instant,
    ) -> CaptureEvidenceBinding {
        CaptureEvidenceBinding {
            scope: self.scope.clone(),
            aggregate: self.aggregate.clone(),
            worktree: self.worktree.clone(),
            content: self.content.clone(),
            losses: self.losses.clone(),
            deadline,
        }
    }
}

/// Obtain the exact obligation first, then the coherent active projection for
/// its product. Scopes are the already-read immutable captured scope rows; the
/// inventory request carries the resolved full identity actually being sealed.
pub(in crate::runtime::close_retirement) async fn read_capture_evidence(
    db: &Database,
    request: &CaptureCloseRetirementInventoryRequest,
    scopes: &[CloseAttemptScope],
) -> Result<Vec<ScopeCaptureEvidence>, String> {
    let obligation = db
        .get_close_obligation(request.attempt_id.as_str())
        .await
        .map_err(|error| error.to_string())?;
    if obligation.attempt_id() != &request.attempt_id
        || obligation.phase() != ClosePhase::RetirementRequested
        || obligation.snapshot() != Some(&request.snapshot)
    {
        return Err("native capture requires the original RetirementRequested snapshot".into());
    }
    let projection = db
        .get_active_close_projection_for_product(obligation.product_conversation_id())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("native capture has no active product projection")?;
    if projection.obligation.attempt_id() != &request.attempt_id
        || projection.obligation.phase() != ClosePhase::RetirementRequested
        || projection.obligation.snapshot() != Some(&request.snapshot)
    {
        return Err("native capture projection is not the original requested attempt".into());
    }
    validate_capture_evidence(
        &request.attempt_id,
        &request.snapshot,
        scopes,
        request,
        &projection.inspections,
        &projection.losses,
    )
}

fn validate_capture_evidence(
    attempt: &CloseAttemptId,
    aggregate: &CloseRetirementSnapshot,
    scopes: &[CloseAttemptScope],
    request: &CaptureCloseRetirementInventoryRequest,
    inspections: &[CloseInspection],
    inspection_losses: &[CloseInspectionLoss],
) -> Result<Vec<ScopeCaptureEvidence>, String> {
    if request.attempt_id != *attempt
        || request.snapshot != *aggregate
        || request.scopes.len() != scopes.len()
        || scopes.is_empty()
    {
        return Err("native capture scope or aggregate mismatch".into());
    }
    phoenix_db::validate_close_retirement_inspection_aggregate(aggregate, inspections)
        .map_err(|error| error.to_string())?;
    let mut result = Vec::with_capacity(scopes.len());
    for captured in scopes {
        if captured.attempt_id != *attempt
            || result
                .iter()
                .any(|prior: &ScopeCaptureEvidence| prior.scope == captured.scope)
        {
            return Err("duplicate or foreign native capture scope".into());
        }
        let sealed = request
            .scopes
            .iter()
            .filter(|item| item.scope == captured.scope)
            .collect::<Vec<_>>();
        if sealed.len() != 1 {
            return Err("missing or duplicate native capture inventory scope".into());
        }
        let worktree = match &captured.captured_worktree {
            None => None,
            Some(CapturedWorktreeIdentity::Resolved(identity)) => Some(identity.clone()),
            Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
                return Err("unresolved native capture worktree".into());
            }
        };
        if sealed[0].inventory.worktree != worktree {
            return Err("native capture full worktree identity differs from inventory".into());
        }
        let matched_inspections = inspections
            .iter()
            .filter(|inspection| inspection.target.scope == captured.scope)
            .collect::<Vec<_>>();
        if matched_inspections.len() != usize::from(worktree.is_some()) {
            return Err("native capture inspection mapping mismatch".into());
        }
        let content = matched_inspections
            .first()
            .map(|inspection| inspection.snapshot.clone());
        if matched_inspections
            .iter()
            .any(|inspection| inspection.attempt_id != *attempt)
        {
            return Err("native capture inspection has foreign attempt".into());
        }
        let mut losses = Vec::new();
        for loss in inspection_losses
            .iter()
            .filter(|loss| loss.scope == captured.scope)
        {
            if loss.attempt_id != *attempt
                || Some(&loss.snapshot) != content.as_ref()
                || losses.contains(&loss.item)
            {
                return Err("native capture loss does not match scope inspection".into());
            }
            losses.push(loss.item.clone());
        }
        result.push(ScopeCaptureEvidence {
            scope: captured.scope.clone(),
            aggregate: aggregate.clone(),
            worktree,
            content,
            losses,
        });
    }
    if request
        .scopes
        .iter()
        .any(|sealed| !scopes.iter().any(|captured| captured.scope == sealed.scope))
        || inspections.iter().any(|inspection: &CloseInspection| {
            !scopes
                .iter()
                .any(|captured| captured.scope == inspection.target.scope)
        })
        || inspection_losses.iter().any(|loss: &CloseInspectionLoss| {
            !scopes.iter().any(|captured| captured.scope == loss.scope)
        })
    {
        return Err("native capture contains an extra inventory, inspection or loss scope".into());
    }
    Ok(result)
}

#[cfg(test)]
pub(in crate::runtime::close_retirement) fn test_scope(
    scope: WorkScopeId,
    aggregate: CloseRetirementSnapshot,
) -> ScopeCaptureEvidence {
    ScopeCaptureEvidence {
        scope,
        aggregate,
        worktree: None,
        content: None,
        losses: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use phoenix_core::domain::close::{
        CloseOwnedResourceInventory, CloseRetirementTarget, GitPathIdentity, WorktreeFingerprint,
        WorktreeId,
    };
    use phoenix_db::CaptureCloseRetirementInventoryScopeRequest;
    use std::collections::BTreeSet;

    fn attempt() -> CloseAttemptId {
        CloseAttemptId::parse("attempt-a").unwrap()
    }

    fn scope(value: &str) -> WorkScopeId {
        WorkScopeId::parse(value).unwrap()
    }

    fn snapshot(value: &str) -> CloseRetirementSnapshot {
        CloseRetirementSnapshot::parse("generation-1", value).unwrap()
    }

    fn aggregate_for(inspections: &[CloseInspection]) -> CloseRetirementSnapshot {
        fn component(
            inspections: &[CloseInspection],
            value: impl Fn(&CloseInspection) -> &str,
        ) -> String {
            let mut ordered = inspections.iter().collect::<Vec<_>>();
            ordered.sort_by(|left, right| left.target.scope.cmp(&right.target.scope));
            if ordered.is_empty() {
                return "no-worktree".into();
            }
            let mut encoded = String::from("v1");
            for inspection in ordered {
                let scope = inspection.target.scope.as_str();
                let value = value(inspection);
                encoded.push_str(&format!("{}:{scope}{}:{value}", scope.len(), value.len()));
            }
            encoded
        }

        let generation = component(inspections, |inspection| inspection.snapshot.generation());
        let fingerprint = component(inspections, |inspection| inspection.snapshot.fingerprint());
        CloseRetirementSnapshot::parse(&generation, &fingerprint).unwrap()
    }

    fn aggregate_without_worktrees() -> CloseRetirementSnapshot {
        CloseRetirementSnapshot::parse("custom-empty-generation", "no-worktree").unwrap()
    }

    fn assert_rejected_as(error: String, expected: &str) {
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error:?}"
        );
    }

    fn validate_error(
        attempt_id: &CloseAttemptId,
        aggregate: &CloseRetirementSnapshot,
        scopes: &[CloseAttemptScope],
        request: &CaptureCloseRetirementInventoryRequest,
        inspections: &[CloseInspection],
        losses: &[CloseInspectionLoss],
    ) -> String {
        match validate(attempt_id, aggregate, scopes, request, inspections, losses) {
            Err(error) => error,
            Ok(_) => panic!("invalid capture unexpectedly validated"),
        }
    }

    fn worktree(fingerprint: &str, locator: &[u8]) -> WorktreeIdentity {
        WorktreeIdentity::from_parts(
            WorktreeId::parse("worktree-id").unwrap(),
            WorktreeFingerprint::parse(fingerprint).unwrap(),
            GitPathIdentity::from_bytes(locator.to_vec()),
        )
    }

    fn inventory(identity: Option<WorktreeIdentity>) -> CloseOwnedResourceInventory {
        CloseOwnedResourceInventory {
            worktree: identity,
            work_scopes: BTreeSet::new(),
            bash_process_groups: BTreeSet::new(),
            tmux_servers: BTreeSet::new(),
            pty_sessions: BTreeSet::new(),
            browser_sessions: BTreeSet::new(),
            equivalent_live_resources: BTreeSet::new(),
        }
    }

    fn request(
        attempt_id: CloseAttemptId,
        aggregate: CloseRetirementSnapshot,
        entries: Vec<(WorkScopeId, Option<WorktreeIdentity>)>,
    ) -> CaptureCloseRetirementInventoryRequest {
        CaptureCloseRetirementInventoryRequest {
            attempt_id,
            snapshot: aggregate,
            scopes: entries
                .into_iter()
                .map(
                    |(scope, identity)| CaptureCloseRetirementInventoryScopeRequest {
                        scope,
                        inventory: inventory(identity),
                    },
                )
                .collect(),
        }
    }

    fn captured_scope(
        attempt_id: CloseAttemptId,
        scope_id: WorkScopeId,
        identity: Option<CapturedWorktreeIdentity>,
    ) -> CloseAttemptScope {
        CloseAttemptScope {
            attempt_id,
            scope: scope_id,
            captured_worktree: identity,
            captured_at: Utc::now(),
        }
    }

    fn inspection(
        attempt_id: CloseAttemptId,
        scope_id: WorkScopeId,
        content: CloseRetirementSnapshot,
    ) -> CloseInspection {
        CloseInspection {
            attempt_id,
            target: CloseRetirementTarget { scope: scope_id },
            snapshot: content,
            inspected_at: Utc::now(),
        }
    }

    fn validate(
        attempt_id: &CloseAttemptId,
        aggregate: &CloseRetirementSnapshot,
        scopes: &[CloseAttemptScope],
        request: &CaptureCloseRetirementInventoryRequest,
        inspections: &[CloseInspection],
        losses: &[CloseInspectionLoss],
    ) -> Result<Vec<ScopeCaptureEvidence>, String> {
        validate_capture_evidence(attempt_id, aggregate, scopes, request, inspections, losses)
    }

    #[test]
    fn accepts_exact_full_identity_including_non_utf8_locator() {
        let attempt = attempt();
        let scope_id = scope("scope-a");
        let identity = worktree("fingerprint-a", b"repo/\xff");
        let content = snapshot("content");
        let inspections = [inspection(
            attempt.clone(),
            scope_id.clone(),
            content.clone(),
        )];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_id.clone(), Some(identity.clone()))],
        );
        let scopes = [captured_scope(
            attempt.clone(),
            scope_id.clone(),
            Some(CapturedWorktreeIdentity::Resolved(identity.clone())),
        )];

        let evidence =
            validate(&attempt, &aggregate, &scopes, &request, &inspections, &[]).unwrap();

        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].worktree, Some(identity));
        assert_eq!(evidence[0].content, Some(content));
    }

    #[test]
    fn rejects_fingerprint_or_locator_inventory_changes() {
        for (captured, inventoried) in [
            (
                worktree("fingerprint-a", b"repo"),
                worktree("fingerprint-b", b"repo"),
            ),
            (
                worktree("fingerprint-a", b"repo-a"),
                worktree("fingerprint-a", b"repo-b"),
            ),
        ] {
            let attempt = attempt();
            let scope_id = scope("scope-a");
            let inspections = [inspection(
                attempt.clone(),
                scope_id.clone(),
                snapshot("content"),
            )];
            let aggregate = aggregate_for(&inspections);
            let request = request(
                attempt.clone(),
                aggregate.clone(),
                vec![(scope_id.clone(), Some(inventoried))],
            );
            let scopes = [captured_scope(
                attempt.clone(),
                scope_id.clone(),
                Some(CapturedWorktreeIdentity::Resolved(captured)),
            )];

            assert_rejected_as(
                validate_error(&attempt, &aggregate, &scopes, &request, &inspections, &[]),
                "full worktree identity differs",
            );
        }
    }

    #[test]
    fn rejects_unresolved_identity_and_accepts_scope_without_worktree() {
        let attempt = attempt();
        let aggregate = aggregate_without_worktrees();
        let unresolved_scope = scope("unresolved");
        let unresolved = [captured_scope(
            attempt.clone(),
            unresolved_scope.clone(),
            Some(CapturedWorktreeIdentity::Unresolved {
                locator: GitPathIdentity::from_bytes(b"repo".to_vec()),
            }),
        )];
        let unresolved_request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(unresolved_scope, None)],
        );
        assert!(validate(
            &attempt,
            &aggregate,
            &unresolved,
            &unresolved_request,
            &[],
            &[]
        )
        .is_err());

        let plain_scope = scope("plain");
        let plain = [captured_scope(attempt.clone(), plain_scope.clone(), None)];
        let plain_request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(plain_scope, None)],
        );
        assert!(validate(&attempt, &aggregate, &plain, &plain_request, &[], &[]).is_ok());
    }

    #[test]
    fn rejects_missing_extra_and_duplicate_inventory_scopes() {
        let attempt = attempt();
        let aggregate = aggregate_without_worktrees();
        let scope_a = scope("scope-a");
        let scope_b = scope("scope-b");
        let captured = [captured_scope(attempt.clone(), scope_a.clone(), None)];

        let missing = request(attempt.clone(), aggregate.clone(), vec![]);
        assert!(validate(&attempt, &aggregate, &captured, &missing, &[], &[]).is_err());

        let extra = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_a.clone(), None), (scope_b, None)],
        );
        assert!(validate(&attempt, &aggregate, &captured, &extra, &[], &[]).is_err());

        let duplicate = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_a.clone(), None), (scope_a.clone(), None)],
        );
        let duplicate_scopes = [
            captured_scope(attempt.clone(), scope_a.clone(), None),
            captured_scope(attempt.clone(), scope_a, None),
        ];
        assert!(validate(
            &attempt,
            &aggregate,
            &duplicate_scopes,
            &duplicate,
            &[],
            &[]
        )
        .is_err());
    }

    #[test]
    fn rejects_foreign_attempt_inspection_and_loss() {
        let attempt = attempt();
        let scope_id = scope("scope-a");
        let identity = worktree("fingerprint", b"repo");
        let scopes = [captured_scope(
            attempt.clone(),
            scope_id.clone(),
            Some(CapturedWorktreeIdentity::Resolved(identity.clone())),
        )];
        let content = snapshot("content");
        let inspections = [inspection(
            attempt.clone(),
            scope_id.clone(),
            content.clone(),
        )];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_id.clone(), Some(identity))],
        );
        let foreign_attempt = CloseAttemptId::parse("attempt-foreign").unwrap();
        assert!(validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &[inspection(
                foreign_attempt.clone(),
                scope_id.clone(),
                content.clone()
            )],
            &[],
        )
        .is_err());
        let loss = CloseInspectionLoss {
            attempt_id: foreign_attempt,
            scope: scope_id,
            snapshot: content,
            item: CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
                b"new-file".to_vec(),
            )),
        };
        assert!(validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &[loss]
        )
        .is_err());
    }

    #[test]
    fn rejects_mismatched_snapshot_and_duplicate_losses() {
        let attempt = attempt();
        let scope_id = scope("scope-a");
        let identity = worktree("fingerprint", b"repo");
        let scopes = [captured_scope(
            attempt.clone(),
            scope_id.clone(),
            Some(CapturedWorktreeIdentity::Resolved(identity.clone())),
        )];
        let content = snapshot("content");
        let inspections = [inspection(
            attempt.clone(),
            scope_id.clone(),
            content.clone(),
        )];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_id.clone(), Some(worktree("fingerprint", b"repo")))],
        );
        let item = CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
            b"new-file".to_vec(),
        ));
        let loss = |snapshot| CloseInspectionLoss {
            attempt_id: attempt.clone(),
            scope: scope_id.clone(),
            snapshot,
            item: item.clone(),
        };
        assert!(validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &[loss(snapshot("different-content"))],
        )
        .is_err());
        assert!(validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &[loss(content.clone()), loss(content)],
        )
        .is_err());
    }

    #[test]
    fn rejects_coordinated_content_and_loss_change_under_original_aggregate() {
        let attempt = attempt();
        let scope_id = scope("scope-a");
        let identity = worktree("fingerprint", b"repo");
        let original = snapshot("original-content");
        let inspections = [inspection(attempt.clone(), scope_id.clone(), original)];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_id.clone(), Some(identity.clone()))],
        );
        let scopes = [captured_scope(
            attempt.clone(),
            scope_id.clone(),
            Some(CapturedWorktreeIdentity::Resolved(identity)),
        )];
        for changed in [
            CloseRetirementSnapshot::parse("changed-generation", "original-content").unwrap(),
            snapshot("changed-content"),
        ] {
            let changed_inspections = [inspection(
                attempt.clone(),
                scope_id.clone(),
                changed.clone(),
            )];
            let changed_losses = [CloseInspectionLoss {
                attempt_id: attempt.clone(),
                scope: scope_id.clone(),
                snapshot: changed,
                item: CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
                    b"new-file".to_vec(),
                )),
            }];
            assert_rejected_as(
                validate_error(
                    &attempt,
                    &aggregate,
                    &scopes,
                    &request,
                    &changed_inspections,
                    &changed_losses,
                ),
                "retained inspection rows do not prove",
            );
        }
    }

    #[test]
    fn original_worktree_selection_preserves_scope_content_losses_and_deadline() {
        let attempt = attempt();
        let scope_a = scope("scope-a");
        let scope_b = scope("scope-b");
        let identity = worktree("fingerprint", b"same-repo");
        let scopes = [
            captured_scope(
                attempt.clone(),
                scope_a.clone(),
                Some(CapturedWorktreeIdentity::Resolved(identity.clone())),
            ),
            captured_scope(
                attempt.clone(),
                scope_b.clone(),
                Some(CapturedWorktreeIdentity::Resolved(identity.clone())),
            ),
        ];
        let inspections = [
            inspection(attempt.clone(), scope_a.clone(), snapshot("content-a")),
            inspection(attempt.clone(), scope_b.clone(), snapshot("content-b")),
        ];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![
                (scope_a.clone(), Some(identity.clone())),
                (scope_b.clone(), Some(identity.clone())),
            ],
        );
        let losses = [CloseInspectionLoss {
            attempt_id: attempt.clone(),
            scope: scope_a.clone(),
            snapshot: inspections[0].snapshot.clone(),
            item: CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
                b"file-a".to_vec(),
            )),
        }];
        let evidence = validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &losses,
        )
        .unwrap();
        let deadline = tokio::time::Instant::now();
        let bound_a = evidence[0].bind(deadline);
        let bound_b = evidence[1].bind(deadline);
        let before_a = bound_a.clone();
        let selected_a = bound_a
            .select_worktree(&scope_a, &aggregate, &identity)
            .unwrap();
        let selected_b = bound_b
            .select_worktree(&scope_b, &aggregate, &identity)
            .unwrap();
        assert_eq!(selected_a.identity, &identity);
        assert_eq!(selected_a.content, &inspections[0].snapshot);
        assert_eq!(selected_b.content, &inspections[1].snapshot);
        assert_ne!(selected_a.content, &aggregate);
        assert_eq!(selected_a.losses, &[losses[0].item.clone()]);
        assert!(selected_b.losses.is_empty());
        assert_eq!(selected_a.deadline, deadline);
        assert_eq!(selected_b.deadline, deadline);
        assert_eq!(bound_a, before_a);
        assert!(bound_a
            .select_worktree(&scope_b, &aggregate, &identity)
            .is_err());
        assert!(bound_a
            .select_worktree(&scope_a, &snapshot("foreign"), &identity)
            .is_err());
        assert!(bound_a
            .select_worktree(&scope_a, &aggregate, &worktree("changed", b"same-repo"))
            .is_err());
        assert!(bound_a
            .select_worktree(&scope_a, &aggregate, &worktree("fingerprint", b"elsewhere"))
            .is_err());
    }

    #[test]
    fn original_worktree_selection_does_not_invent_missing_evidence() {
        let scope_id = scope("plain");
        let aggregate = aggregate_without_worktrees();
        let deadline = tokio::time::Instant::now();
        let mut bound = test_scope(scope_id.clone(), aggregate.clone()).bind(deadline);
        let identity = worktree("fingerprint", b"repo");
        assert!(bound
            .select_worktree(&scope_id, &aggregate, &identity)
            .is_err());
        bound.worktree = Some(identity.clone());
        assert!(bound
            .select_worktree(&scope_id, &aggregate, &identity)
            .is_err());
    }

    #[test]
    fn exact_repeats_bind_same_evidence_and_preserve_deadline() {
        let attempt = attempt();
        let scope_id = scope("scope-a");
        let identity = worktree("fingerprint", b"repo");
        let content = snapshot("content");
        let item = CloseLossItem::UntrackedNonIgnoredPath(GitPathIdentity::from_bytes(
            b"new-file".to_vec(),
        ));
        let inspections = [inspection(
            attempt.clone(),
            scope_id.clone(),
            content.clone(),
        )];
        let aggregate = aggregate_for(&inspections);
        let request = request(
            attempt.clone(),
            aggregate.clone(),
            vec![(scope_id.clone(), Some(identity.clone()))],
        );
        let scopes = [captured_scope(
            attempt.clone(),
            scope_id.clone(),
            Some(CapturedWorktreeIdentity::Resolved(identity)),
        )];
        let losses = [CloseInspectionLoss {
            attempt_id: attempt.clone(),
            scope: scope_id,
            snapshot: content.clone(),
            item: item.clone(),
        }];
        let first = validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &losses,
        )
        .unwrap();
        let repeated = validate(
            &attempt,
            &aggregate,
            &scopes,
            &request,
            &inspections,
            &losses,
        )
        .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let first_binding = first[0].bind(deadline);
        let repeated_binding = repeated[0].bind(deadline);

        assert_eq!(first.len(), repeated.len());
        assert_eq!(first[0].scope, repeated[0].scope);
        assert_eq!(first[0].aggregate, repeated[0].aggregate);
        assert_eq!(first[0].worktree, repeated[0].worktree);
        assert_eq!(first[0].content, repeated[0].content);
        assert_eq!(first[0].losses, repeated[0].losses);
        assert_eq!(first_binding, repeated_binding);
        assert_eq!(first_binding.deadline(), deadline);
        assert!(first_binding.verifies_request(&aggregate));
        assert_eq!(first_binding.snapshot(), &aggregate);
        assert_eq!(first[0].losses, vec![item]);
    }
}
