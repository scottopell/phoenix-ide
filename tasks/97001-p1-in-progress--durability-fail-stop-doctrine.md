# Define Phoenix durability and local SQLite fail-stop doctrine

## Objective

Establish the normative durability contract and project ADR for Phoenix’s local SQLite authority boundary. This is a specifications-only workstream: do not modify production code, tests, or the independent SQLite telemetry workstream.

Phoenix may stop at almost any instruction. After restart, SQLite must describe one coherent state and expose every unfinished durable obligation. Correctness must not depend on retaining process-local workers, tasks, timers, kicks, runtime objects, provider tasks, SSE connections, replay rings, or queued events.

When an authoritative local SQLite command errors, Phoenix may issue one exact authority query. A typed classification (commit, non-commit, replay/already-terminal, stale authority, or another exact result) governs continuation. If the exact query is unavailable or cannot establish the durable fact needed to continue safely, the persistence path is unhealthy and Phoenix must fail-stop; epistemic uncertainty must not become conversation/runtime state.

## Work boundaries

- Specifications and ADRs only; no production implementation.
- Do not touch the SQLite telemetry owner’s branch, task, PR, or roadmap workstream.
- Do not continue or cherry-pick PRs #683 or #687; use them only as historical architectural evidence.
- Do not introduce persisted SSE/outbox design, an ambiguity state, automatic rollback, live SQLite replacement, generic cross-version promises, or detailed implementation design.
- Treat genuine ambiguous external outcomes (providers, GitHub, and remote tools/services) as feature-owned durability problems distinct from same-process SQLite classification failure.

## Plan

### 1. Establish exact coordination and base

- Start with the `phoenix-development` workflow.
- Read GitHub Issue #651’s generated roadmap body and schema-defining first comment before relying on the handoff.
- Fetch current refs and verify exact `origin/main`, including merged PRs #684 and #685 and the merge state of PR #688.
- Create the durable task atomically with `taskmd new`, then create one fresh owned branch from exact current `origin/main`; record task ID, branch, and base SHA. If this approved proposal itself becomes the repository task, reconcile it through `taskmd` rather than creating a duplicate.
- Register only this owned workstream on the roadmap, following the reducer/reaction verification protocol.

### 2. Audit current authorities and failure model

- Read the exact current versions of:
  - `specs/durable-workflows/requirements.md` and related executive/Allium artifacts;
  - `specs/bedrock/requirements.md` and related executive/Allium artifacts;
  - `specs/compatibility/requirements.md` and executive material;
  - `specs/adrs/README.md`, relevant prior ADRs, and `specs/AUTHORING.md`.
- Inspect the current direct-turn state machine, persistence boundary, and runtime publication/adoption path only enough to describe the normative invariant; do not implement changes.
- Inspect closed PRs #683 and #687 as historical evidence and avoid assigning any stale/unmerged ADR number.
- Produce a concise clause map identifying the owning artifact for each requirement, existing contradictions, and any genuine unresolved product choice.

### 3. Run a full read-only review panel

Before editing or before finalizing edits, obtain independent read-only reviews covering:

1. durable-workflow authority and recovery obligations;
2. Bedrock runtime/state-machine correctness;
3. SQLite transaction and process failure semantics;
4. compatibility boundaries and timeless spec authoring;
5. YAGNI/complexity skepticism.

Synthesize a decision memo. Resolve all real open questions; ask the user only if evidence leaves an actual product choice. Leave no unresolved prose questions in normative artifacts.

### 4. Update timeless normative requirements

- **Durable workflows:** place the durability doctrine prominently near the foundation; make committed rows plus durable time sufficient to discover unfinished obligations; declare process-local mechanisms disposable; distinguish external ambiguity from failure to classify local SQLite; prevent adapters, SSE, and runtime projections from becoming parallel authority.
- **Bedrock:** own the narrow fail-stop trigger and controlled fatal shutdown contract:
  - trigger only when an authoritative command and its one exact authority query cannot classify the needed durable fact, or when a critical authority-boundary task panics, unexpectedly exits, or is cancelled without delivering a typed durable result;
  - stop admission and new semantic publication;
  - avoid DB-backed cleanup depending on the suspect path;
  - flush logs/traces best-effort, exit nonzero promptly, and abort if bounded shutdown stalls;
  - rely on supervisor restart with backoff;
  - distinguish fatal persistence-health shutdown from ordinary SIGTERM/deploy shutdown and abrupt SIGKILL/OOM/host failure;
  - prohibit epistemic uncertainty in `ConvState`;
  - make SQLite authoritative across restart while runtime/SSE/UI identity remains a disposable projection.
- Enumerate nonfatal typed outcomes: stale generation, exact replay/already-terminal, known rollback or constraint rejection, expected lock contention with known non-commit, pool timeout before transaction start, and command error followed by successful exact classification.
- **Compatibility:** concisely record non-guarantees for healing an unhealthy live SQLite connection, preserving runtime/SSE identity across persistence-health failure, and live-resource replacement unless explicitly required. Keep operational behavior owned by Bedrock.
- State the critical publication invariant: a runtime must not externally represent or publish durable semantic state before its owning SQLite transaction commits; proposed transition plans/state must remain structurally distinct from committed runtime state; direct-turn materialization should occur before runtime publication/adoption.
- Touch direct-chat/Bedrock Allium only if EARS requirements plus ADR cannot express required behavioral precision. Never add `Ambiguous`; if needed, model typed authoritative outcomes outside semantic conversation state and fail-stop as a system boundary.

### 5. Add the project ADR and status updates

- Allocate the next valid ADR number from current `specs/adrs/README.md` and update the index.
- Record context, alternatives, decision, and consequences:
  - authority loss for local SQLite uses process fail-stop/restart;
  - same-process SQLite is not modeled as a distributed external side effect;
  - process-local observer/runtime continuity is sacrificed;
  - genuine external ambiguity remains feature-owned;
  - restart is a primary correctness mechanism;
  - prior conversation-local recovery attempts expanded coordination across too many independent in-memory owners.
- Link closed PRs #683 and #687 as historical evidence without progress claims or conflicting ADR numbering.
- Update executive/status documents only for appropriate current-reality tracking; keep requirements and Allium timeless and status-free.
- After doctrine is settled, create a separate concrete implementation follow-up task if the direct-turn audit finds the publication/commit invariant is not structurally enforced. Do not implement it in this workstream.

### 6. Validate, review, and deliver without merging

- Run the complete `specs/AUTHORING.md` preflight.
- Validate every touched Allium file and require no new warnings/errors.
- Run `./dev.py tasks validate`, `git diff --check`, and `./dev.py check --all`.
- Review the final diff against the clause map, decision memo, timelessness rules, and complexity exclusions.
- Commit logical units, push the owned branch, open a PR, and request Codex exact-head review.
- Enter a bounded exact-head loop: fix useful findings minimally, rerun focused/full validation, push, and repeat until exact-head CI and Codex review are clean.
- Do not merge. Report the PR ready for user review with task ID, branch, exact base SHA, clause map, panel memo, changed normative artifacts, ADR number, any follow-up task, validation evidence, PR URL, and exact-head review status.

## Acceptance criteria

- The durability and fail-stop boundaries are plain-English, testable, timeless, and owned by the correct specs.
- No semantic `Ambiguous`, `Unknown`, `MaybeCommitted`, or `RecoveryUncertain` runtime/conversation state is introduced.
- Runtime/SSE/UI projections are non-authoritative without making unrelated persistence-frequency claims.
- Fatal and nonfatal SQLite outcomes, critical task supervision, and shutdown classes are clearly distinguished.
- The direct-turn commit-before-publication invariant is captured normatively, with implementation deferred to a separate task if needed.
- A correctly numbered ADR records the decision and alternatives without overdesigning implementation.
- Full authoring, task, diff, Allium (if applicable), and repository validation pass.
- Exact-head CI and Codex review are clean; the PR remains unmerged for user review.
