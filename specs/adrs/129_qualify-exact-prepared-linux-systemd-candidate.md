# ADR-129: Qualify exact prepared Linux systemd candidate

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-COMP-016, REQ-PD-002/003/012/021/024/025/029; `ProtectedSystemdPreparedDeployment`

## Context

The fixed application candidate `237100c79d9ce672867b284167a099fa0b7a2bcf` contains the required runtime acknowledgement correction and the complete migration registry through128. The previously qualified published Linux candidate is a different commit with different binary and registry-source hashes. Reusing its release identity would make two distinct artifacts represent one authority, while publishing a replacement tag or compiling on the deployment host would weaken the already bounded production operation.

The homelab predecessor has the complete ordered source ledger through65. Its transition remains irreversible once candidate database writes begin, so artifact provenance does not replace the backup, rehearsal, persistent inhibit, exclusive ownership, durable marker and candidate-only recovery guarantees decided by ADR-116 and ADR-117. The preparation workflow and deployment helper are controller code produced after the application candidate; their identity must be independent of the candidate and exact at dispatch and activation.

## Options considered

1. **Repin the existing published RC4 contract to the fixed commit.** Rejected because no immutable published release identifies those bytes, and changing the old pins would erase the distinction between the qualified published artifact and the new candidate.
2. **Add a generic Linux prepared-artifact mode.** Rejected because arbitrary commits, architectures, registries, baselines and receipts would create an open compatibility promise and make unsupported combinations representable.
3. **Qualify one exact private Linux preparation as a sibling source variant and reuse the protected systemd lifecycle.** Chosen because it preserves the published route, binds one fixed candidate and producer/controller, and changes only source admission rather than the irreversible-transition algorithm.

## Decision

Choose option 3. REQ-PD-029 is the immutable prepared-source authority: application commit `237100c79d9ce672867b284167a099fa0b7a2bcf`, embedded version `0.13.0-rc.4`, target `x86_64-unknown-linux-musl`, registry-source SHA-256 `a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6`, operation `prepare-incident-237100-linux-x86_64`, and its exact payload and receipt basenames. The prepared directory contains only those two nonsymlink regular files. The closed receipt binds the exact candidate, target, registry, payload hash, successful clean/static/embedded-identity checks, workflow run and attempt, `workflow_dispatch`, `refs/heads/main`, and the full workflow-controller commit.

The workflow-controller commit is a separately selected full commit different from the application candidate. It must equal the deployment controller/helper commit. Candidate registry bytes come from the exact application git object; helper bytes come from the exact controller git object. The controller copies receipt and payload through no-follow descriptors into private staging before parsing or executing either path, validates the complete embedded identity and receipt hash, and passes distinct retained candidate, registry, helper and receipt artifacts to the privileged helper. The helper independently re-parses and cross-checks those artifacts before disruption. Local receipt JSON is binding evidence but not authenticated GitHub provenance; operator admission independently establishes the successful protected run/source/artifact association.

Represent this authority as `ProtectedSystemdPreparedDeployment`, not `published_release` and not the launchd paired prepared variant. Use protected handoff protocol 3 for its required receipt field. Keep the exact published systemd route and protocol 2 unchanged. No release tag, release asset metadata, local build, host compilation or public publication represents the prepared source.

Reuse ADR-116 and ADR-117's protected lifecycle without weakening it: exact prefix65 source admission, preserved installed configuration, persistent verified service/socket inhibits, both-unit stop and exclusive database ownership, pristine backup, isolated exact-candidate rehearsal, stopped-snapshot equality, and a durable live `migration_started` boundary. At or after that boundary, never restore the backup or predecessor runtime, start the old binary, or substitute another candidate, registry, receipt, helper or controller. Explicit recovery may use only the exact retained prepared candidate and same-registry prefix65..128; exact target128 may finalize without rerunning migration. Invalid or unavailable evidence remains stopped, inhibited and claimed.

The preparation workflow, installer/controller source, hosted regressions, exact artifact production, disposable real-systemd/reboot journey and final host admission remain separate qualification gates. Source availability, green CI, a retained command, or this decision does not authorize workflow dispatch or production execution.

## Consequences

- **Positive:** The fixed application can be qualified without mislabeling it as the older published release or compiling on the production host.
- **Positive:** Application, workflow/controller, registry, receipt and payload identities remain distinct and converge only through explicit cross-checks.
- **Negative:** The artifact is useful only with the exact merged controller commit recorded by its receipt. A controller change requires a new preparation; an artifact from another run or commit is ineligible.
- **Negative:** A post-marker failure can leave production stopped behind persistent inhibits and a retained claim; recovery remains exact-candidate-only.
- **Neutral:** The existing published RC4 qualification remains available under its original pins. No generic prepared Linux, downgrade, same-schema replacement or cross-candidate recovery guarantee is created.

## References

- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-016.
- [Production deployment requirements](../production-deployment/requirements.md), REQ-PD-029, and `production-deployment.allium`.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md), explicit compatibility policy.
- [ADR-116](116_protect-exact-65-to-128-systemd-published-release-activation.md), protected systemd lifecycle.
- [ADR-117](117_recover-interrupted-pre-marker-systemd-activation.md), bounded interrupted recovery.
- `ProdDeployControllerOptions::require_systemd_current` and `_prepare_linux_artifact` in `dev.py`.
- `validate_mode`, `validate_manifest`, `run_current_migration` and `recover_current` in `scripts/systemd_deploy_helper.py`.
