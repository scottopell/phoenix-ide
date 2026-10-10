# ADR-088: Close stops on failure and separates cleanup from lifecycle

- **Status:** Accepted
- **Date:** 2026-10-08
- **Affects:** REQ-BED-029, REQ-BED-030A, REQ-WL-002/002a/002b/002c/002d/004, REQ-PROJ-028a, REQ-WAB-007; `CloseObligation`, `CloseRun`, `CloseRunFailure`, unified Global delivery
- **Supersedes:** ADR-026's all-resources-retired prerequisite for History and Open needs-repair retry; ADR-040's automatic restart retirement; ADR-041's successful-cleanup-only History finalization; ADR-042's interrupted-attempt recovery/NeedsRepair consequences; ADR-080's continuation after a failed fsmonitor stop. Their unrelated ownership and identity safety decisions remain in force.

## Context

The Close contract has accumulated automatic convergence machinery: restart resumes incomplete retirement, failures remain Open in NeedsRepair, and retry reuses the same execution. That makes one user Close request authorize continuing destructive work after a failure or crash, and conflates a safely ended conversation with fully retired filesystem resources. The user has selected a bounded alternative: one normal execution, stop at first failure, startup observation only, and explicit safe retry. Historical recovery evidence is not acceptance evidence for this changed contract, and ancient quarantined-worktree/database cleanup is not a release goal.

Resource identity and deletion safety remain essential. The WorkScope gate, one ordinary owner, live process permits, exact tmux identity, private retirement namespace, exact loss inspection, reconstructibility proof, and branch/PR preservation do not depend on automatic recovery.

## Options considered

1. **Retain automatic convergence and Open NeedsRepair.** Preserve ADR-026/040/041/042's recovery model and same-execution retry. It may eventually clean resources, but expands authority after failure and keeps safely ended conversations Open because unrelated cleanup remains incomplete.
2. **Treat every failed Close as fully successful History.** Simplifies lifecycle, but fabricates shutdown/cleanup success and can hide live execution or unique retained work.
3. **Stop each run at first failure, distinguish shutdown from resource retirement, and permit only explicit fresh safe retry.** Proven conversation-and-process shutdown enters History with visible cleanup attention; uncertain shutdown remains Open CloseIncomplete. Every failure durably informs Global without granting repair authority.

## Decision

Choose option 3. The original Close operation retains its durable identity and authority. Its normal execution is run ordinal 1. First failure or interruption permanently stops that run and fences later settlement, cleanup, repair, and retry effects. Startup reads and records observations only; it never drives Close recovery.

Positive proof that both conversation execution and owned processes stopped permits lifecycle History even if filesystem/resource cleanup failed. The atomic outcome records conspicuous `cleanup_attention` and exact residuals without claiming resource retirement. Uncertain shutdown remains Open with typed `CloseIncomplete`. History never reopens as a cleanup-retry side effect.

Each distinct run-bound failure atomically creates a mandatory once-per-failure Global event through the unified durable delivery/outbox path, regardless of watch enrollment or History transition. Duplicate observation or an overlapping watch must not duplicate that event. Delivery receipt means notification acceptance, not repair approval or successful cleanup.

Global may investigate read-only. An explicit user or Global safe retry needs fresh proof that the failed precondition resolved and the exact remaining effects are safe under the original Close authority. It creates a new monotonically increasing run ordinal; it cannot resume a stopped run, replay completed effects, repeat an unresolved failure, or expand targets/effect kinds. Changed risk, unique or uncertain discard, expanded effects, or DB surgery requires a concrete proposal and separately approved authority, not a generic retry.

Automatic deletion is limited to freshly proven reconstructible disposable state. Unique or uncertain work is preserved until an exact explicit discard decision. Close never deletes or otherwise mutates branches or PRs and creates no automatic recovery artifact. Identity uncertainty leaves resources untouched. A failed bound-fsmonitor stop ends the run rather than continuing into quarantine/scan.

## Consequences

- **Positive:** One user Close request has bounded execution authority; restart does not silently extend it.
- **Positive:** History communicates lifecycle completion without disguising residual cleanup; live or uncertain shutdown cannot masquerade as ended work.
- **Positive:** Global receives durable actionable failure evidence even for unwatched sources, through one delivery authority.
- **Positive:** Fresh run ordinals preserve failure history and reject stale progress without abandoning the original user authority.
- **Negative:** Failed/interrupted resource retirement can leave durable leftovers indefinitely until explicit safe action is admitted.
- **Negative:** Implementations and qualification must separate shutdown proof, lifecycle finalization, resource disposition, and delivery acceptance; old-contract green checks cannot qualify the new behavior.
- **Neutral:** This decision grants no production execution, general recovery framework, compatibility expansion, discard, or database-repair authority.

## References

- ADR-026, ADR-040, ADR-041, ADR-042, ADR-080: superseded Close consequences; retained authority/identity safeguards
- ADR-070: trusted input provenance and explicit Global watches; mandatory failure delivery is an exception to watch enrollment, not a second messaging plane
- `specs/bedrock/requirements.md`, `specs/bedrock/bedrock.allium`
- `specs/work-lifecycle/requirements.md`, `specs/work-lifecycle/work-lifecycle.allium`
- `specs/work-lifecycle/executive.md`: candidate acceptance matrix and validation status
