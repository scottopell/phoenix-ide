# Support multiple PR deliverables through durable task-branch observation

## Objective

Allow one Work or Branch conversation to produce and operate on multiple stacked or sibling pull requests while preserving one owning Phoenix task/worktree lifecycle.

Phoenix shall durably record branches observed as the settled HEAD of the conversation worktree at supported reconciliation boundaries. Those observed branches become candidate PR heads. Phoenix shall discover and retain PR associations for the candidate branch set, expose an explicit active PR for PR-specific surfaces, and keep the existing aggregate workspace diff distinct from a selected PR diff.

The first implementation is intentionally bash-centered. It does not attempt to observe branch transitions that occur only transiently inside one compound command or script, nor changes made through tmux, an external terminal, or an IDE.

## Product contract

Phoenix automatically discovers PRs associated with branches observed at reconciliation boundaries and durably retains both the branch observations and discovered PR associations. Branch states that exist only within one process execution are not observable. An agent can recover by revisiting each PR head branch in a separate bash call; each settled branch is then observed and used for discovery.

One Phoenix task remains the owner of the worktree. Associated branches and PRs are deliverables of that task, not independent Phoenix tasks or lifecycle owners.

## Required behavior

### 1. Durable observed task branches

Introduce a normalized persisted representation of branches observed for a WorkScope. Each observation must identify at least:

- the owning WorkScope;
- repository identity;
- branch name;
- first and last observed HEAD object IDs;
- first and last observation timestamps.

The representation is an historical observation fact: Phoenix observed this branch as the settled HEAD of this worktree. It must not claim exclusive branch ownership or make branch existence permanent.

The schema and types must make repository identity part of branch identity. A branch name alone is insufficient.

The conversation's known base branch must not become a task-branch discovery candidate merely because it was checked out. Candidate qualification must also reject unavailable/detached/unborn states as appropriate and avoid treating a branch with no task-relative work as a deliverable solely because it was visited. Specify and test the exact qualification rule before implementation.

Do not embed the branch collection in an existing JSON column. Model it as rows with schema-enforced identity and required fields.

### 2. Bash lifecycle reconciliation

Use the existing work-scope-keyed bash lifecycle path as the primary invalidation boundary:

```text
BashHandleRegistry terminal lifecycle edge
  -> schedule WorkScope Git reconciliation
  -> observe final worktree HEAD
  -> upsert qualified observed branch
  -> refresh PR discovery candidates
  -> persist PR observations
  -> derive active PR
  -> broadcast the resulting WorkScope/conversation update
```

Reconciliation must run when the bash wrapper process reaches a terminal state, including handles whose spawning tool call returned before the process completed.

Observe authoritative Git state rather than parsing command strings, shell traces, or stdout. The local observation type must distinguish a named branch from detached HEAD, unborn HEAD, and unavailable/error states. Record the HEAD object ID as well as the branch identity.

A single command may switch branches multiple times. Phoenix observes only the final settled state at the process lifecycle boundary. It must not promise or simulate intermediate command observation.

Multiple lifecycle edges for one WorkScope may overlap. Reconciliation must be idempotent, deduplicated per scope, and resistant to stale asynchronous results. A slower refresh triggered by an older local-head observation may persist valid association history, but it must not overwrite active-PR inference based on a newer observation.

### 3. Branch-first PR discovery

PR discovery must use the durable candidate branch set rather than only `ConvMode.branch_name` or only the currently checked-out branch.

A relevant reconciliation shall:

1. observe and persist the current qualified settled branch;
2. query bounded GitHub PR status for candidate task-branch heads according to a documented refresh policy;
3. persist every returned PR association using the existing WorkScope-owned association model;
4. retain associations when the worktree later checks out another branch or the local branch is deleted;
5. refresh already-known PRs by durable repository/PR identity where appropriate, rather than requiring their branch to remain checked out.

PR lookup must not be permanently skipped merely because branch name and HEAD OID are unchanged. Creating a PR does not necessarily change local Git state. Repeated lifecycle events, an explicit PR-aware read, and existing bounded background refresh may all trigger a refresh subject to deduplication/freshness controls.

Do not parse `gh` commands or output as the correctness mechanism. Do not scan every repository branch and attach every matching PR.

### 4. Explicit active PR with smart inference

Represent the PR targeted by PR-specific surfaces as an explicit domain concept separate from:

- all associated PRs;
- the current checked-out branch;
- the owning task/worktree;
- the existing ranked primary-PR compatibility projection.

The model must distinguish a user-pinned selection from an inferred selection so automatic reconciliation cannot silently overwrite deliberate user intent.

Inference order:

1. retain a valid pinned PR;
2. otherwise select the unique actionable associated PR whose head matches the latest settled observed branch;
3. otherwise select the only actionable associated PR across the task;
4. otherwise retain a still-valid prior inferred selection when doing so does not contradict newer authoritative state;
5. otherwise leave PR-specific actions ambiguous/unavailable until an active PR can be selected.

Open PRs are actionable; draft handling must be specified consistently with existing work-action semantics. Merged and closed PRs remain available as history but must not clutter the normal choice when only one actionable PR remains.

Use complete repository plus PR-number identity. Do not persist only a bare PR number.

### 5. Plural PR API and UI

Expose all associated PR summaries for a conversation/work scope together with:

- active PR identity, if any;
- whether selection is inferred or pinned;
- the latest observed branch;
- sufficient per-PR state for stacked and sibling deliverables.

Allow the user to select/pin an associated PR and to resume automatic inference. Scope must be derived from the conversation on the server; do not expose raw WorkScope construction as a user or agent responsibility.

The StateBar PR identity, work-action links, checks, feedback status/freshness, and Address Feedback must all target the same explicit active PR. Labels for destructive or PR-specific actions must identify the PR number so targeting is never implicit.

When there is one actionable PR, select it without offering resolved PRs as competing normal choices. When multiple actionable PRs remain and inference is ambiguous, show the ambiguity rather than silently choosing by recency.

### 6. PR-specific status and feedback targeting

Refactor singular PR status and auto-fix flows so an operation carries explicit PR identity after active selection.

Address Feedback must capture context for exactly the active/selected PR. The existing per-PR feedback baseline and artifact machinery should be reused. Add regression coverage proving that selecting PR A cannot fetch checks, feedback, freshness, or context for PR B.

Retain the existing ranked primary-PR derivation only where needed for migration/backward compatibility. It must no longer be the hidden authority for active multi-PR actions.

### 7. Workspace diff versus PR diff

Preserve the existing conversation/worktree diff as a task-level workspace comparison against the conversation's stored base branch. It must not silently change meaning when active PR changes.

Add a distinct PR-diff operation for the active PR using that PR's base/head comparison, including stacked PRs whose base is another task branch.

The UI must label the two comparisons unambiguously, for example:

- `Workspace Diff` — conversation base to current worktree;
- `PR #102 Diff` — selected PR base to selected PR head.

### 8. Task lifecycle and cleanup

Keep one owning task/brief and one worktree lifecycle. Multiple PRs are task deliverables, not separate task files or subtasks.

Cleanup remains an explicit task/worktree action. Mixed PR states must be summarized to the user, but open/draft PRs must not automatically create a new implicit lifecycle or make feedback freshness a cleanup gate. Cleanup must not mutate, close, or merge associated PRs.

Update lifecycle behavior and specifications wherever they currently consume one primary PR as though it were the only associated PR.

## Recovery workflow for unobservable transient branches

If one compound command or script creates a stack while moving through several branches, Phoenix may observe only its final branch. The supported recovery is:

```text
bash call 1: check out PR head A and exit
  -> Phoenix observes A and discovers its PR
bash call 2: check out PR head B and exit
  -> Phoenix observes B and discovers its PR
bash call 3: check out the desired active head and exit
  -> Phoenix observes it and infers the matching active PR
```

No new `workscope_*`, branch-registration, or PR-registration agent tool is required for this task.

## Explicit non-goals

- Observing intermediate branch transitions inside one bash process.
- Parsing shell commands, shell tracing, scripts, or `gh` output to infer mutations.
- Git-hook installation or modification of user hook configuration.
- Tmux, in-app interactive terminal, external terminal, IDE, or filesystem-watcher observation.
- A user-facing manual `Attach PR` UI.
- An agent-facing WorkScope namespace or explicit PR-registration tool.
- Mirroring GitHub's PR model in Phoenix.
- Multiple independently owning Phoenix task files in one worktree.
- Automatic merge, close, retarget, or cleanup decisions across a PR set.

## Specifications

Before implementation, update the normative artifacts that currently define a single primary PR for status/actions:

- `specs/pr-association/requirements.md` and its current-reality executive;
- relevant work-lifecycle requirements and Allium behavior;
- work-actions-bar requirements;
- projects/task lifecycle specifications if their cardinality language requires clarification;
- bash requirements for the terminal-lifecycle reconciliation contract.

Add an ADR for the decision to use durable settled-branch observations plus explicit active-PR selection, including why Phoenix does not parse commands, install Git hooks, expose WorkScope to agents, or attempt intermediate observation.

If an Allium spec owns the affected lifecycle or reconciliation flow, update it with ordering, stale-result rules, pinned-versus-inferred selection behavior, and invariants. Do not leave unresolved questions in normative specifications.

Run the spec authoring pre-flight checklist in `specs/AUTHORING.md` before pushing specification changes.

## Implementation constraints

- Follow correct-by-construction domain modeling: distinguish observed branches, PR associations, active selection provenance, and Git observation failures with types rather than optional strings and comments.
- Normalize persisted branch collections into child rows; do not add branch arrays to serialized conversation blobs.
- Keep local Git observation cheap and separate from slower remote PR refresh.
- Deduplicate refresh work by WorkScope and suppress stale projection updates with an observation generation or equivalent structural mechanism.
- Capability gaps and failed GitHub refreshes must be logged at debug level or above without blocking ordinary conversation work.
- Preserve existing provider/API behavior when GitHub is unavailable; cached associations and selection must degrade honestly rather than disappear or silently retarget.
- Regenerate checked-in TypeScript wire types through `./dev.py codegen` when Rust wire contracts change.

## Verification

Add focused unit, integration, state-machine/spec, and UI tests covering at least:

1. A settled non-base branch is durably observed with repository identity and HEAD OID.
2. Checking out the conversation base does not register it as a task deliverable candidate.
3. Detached, unborn, missing-worktree, and Git-error observations remain distinguishable and do not create branch rows.
4. A background bash handle reconciles only when its process reaches a terminal state.
5. Multiple checkouts inside one bash call produce one final observation.
6. Separate bash calls in one LLM turn produce separate branch observations.
7. A branch observed before PR creation discovers the PR on a later reconciliation even when branch and HEAD OID are unchanged.
8. Revisiting transient stack branches causes each PR to be discovered and retained.
9. Switching away from a known branch does not remove its PR association.
10. Older overlapping refresh results cannot replace active inference based on a newer observed branch.
11. A pinned PR survives branch switches and automatic refreshes until cleared or invalidated.
12. With one actionable associated PR, Phoenix selects it without requiring a chooser among merged/closed history.
13. With multiple ambiguous actionable PRs, Phoenix does not silently select by recency.
14. Address Feedback, checks, freshness, links, and PR diff all use the same explicit active PR.
15. Workspace diff remains unchanged when active PR changes.
16. A stacked PR diff uses the selected PR's actual base/head rather than the conversation base.
17. Cleanup summarizes mixed PR states but remains an explicit worktree action and does not mutate PRs.
18. GitHub unavailability preserves honest cached plural state and does not retarget actions.

Run `./dev.py check` after implementation and the relevant targeted Rust and UI test suites during development.

## Delivery approach

Deliver in reviewable, shippable increments where practical:

1. specifications, ADR, normalized observed-branch persistence, and local Git observation;
2. bash terminal-edge reconciliation and branch-first plural PR discovery;
3. explicit active-PR domain/API model and singular-flow migration;
4. plural PR selection UI and explicitly targeted feedback/status actions;
5. distinct PR diff plus task-level multi-PR lifecycle presentation.

Each increment must preserve existing single-PR behavior and keep the worktree usable when GitHub observation is unavailable.
