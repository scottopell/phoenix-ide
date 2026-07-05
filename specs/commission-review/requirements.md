# Commission Review

## User Story

As an agent completing a Phoenix task, I need to request an independent review of
my active work so that correctness, security, and regression risks are checked
before I hand the work back to the user.

As a user supervising agent work, I want Phoenix to show me what the agent wants
reviewed and why the review is worth the token spend so that I can approve large
LLM costs deliberately instead of discovering them after the fact.

As a Phoenix operator, I want review to use the same configured model stack as
normal Phoenix conversations so that review works wherever Phoenix already works
and does not require separate credentials or provider setup.

## Requirements

### REQ-CR-001: Review Active Work Without External Setup

WHEN an agent requests an independent code review
THE SYSTEM SHALL use Phoenix's configured LLM provider selection to perform the
review
AND SHALL NOT require the user or agent to provide external review-service
credentials

**Rationale:** Users already trust Phoenix's configured model stack. Requiring a
separate CLI or provider account makes review harder to use and creates a second
place for credentials, quotas, and failures to drift from the active Phoenix
conversation.

---

### REQ-CR-002: Justify Large Review Spend

WHEN an agent requests commission review
THE SYSTEM SHALL require the request to include a non-empty executive brief
explaining why the current work is ready for review and why review is useful now

IF the executive brief is missing or empty
THE SYSTEM SHALL reject the request before collecting review material or calling
an LLM

**Rationale:** Review can be expensive. A concise justification helps the user
make a cost decision and forces the agent to confirm that the work has reached a
review-worthy point.

---

### REQ-CR-003: Require Human Approval Before Review Execution

WHEN an agent requests commission review
THE SYSTEM SHALL present the inferred review scope and spend justification to the
user for approval before performing LLM review work

IF the user rejects the request
THE SYSTEM SHALL return a structured rejected result to the agent
AND SHALL NOT call the review LLM

**Rationale:** Users should control high-cost review actions. Approval before
execution prevents accidental token spend while still letting agents ask for
review at the moment it is most valuable.

---

### REQ-CR-004: Infer the Review Target

WHEN commission review is requested from a git-aware task or worktree
THE SYSTEM SHALL infer the review target from the active conversation and
worktree state
AND SHALL NOT require the agent to supply refs, commits, or diff commands for the
normal review path

WHEN commission review is requested from a direct conversation inside a git
repository
THE SYSTEM SHALL review committed changes on the current HEAD against the fetched
origin ref for the approved base branch, or against the fetched origin default
branch tip when no base branch was approved

**Rationale:** Agents should not need to rebuild Phoenix's knowledge of the
active task. Inferring the target avoids reviewing the wrong branch or comparing
against an ad hoc local ref because of hand-written diff plumbing.

---

### REQ-CR-005: Refuse Dirty Working Trees

WHILE the review target has uncommitted or untracked changes
THE SYSTEM SHALL reject the request with an actionable explanation
AND SHALL NOT call the review LLM

THE SYSTEM SHALL NOT provide a dirty-worktree opt-in or include uncommitted
changes in the review diff

**Rationale:** Commission review is intended for reproducible review of committed
work. Dirty reviews can mix intentional task work with scratch edits, local debug
changes, or generated files, making findings difficult to reproduce or trust.

---

### REQ-CR-006: Compare Against the Approved Origin Base

WHEN commission review collects a diff for an approved base branch
THE SYSTEM SHALL compare the current HEAD against the fetched origin ref for that
approved base branch

WHEN no base branch was approved
THE SYSTEM SHALL compare the current HEAD against the fetched origin default
branch tip

IF the required origin ref is unavailable
THE SYSTEM SHALL reject the request with an actionable explanation
AND SHALL NOT call the review LLM

**Rationale:** Reviewing against the fetched remote base avoids stale local
branch refs while keeping the executed review scope identical to the scope the
user approved.

---

### REQ-CR-007: Hide Review Where Phoenix Cannot Infer Scope

WHILE Phoenix cannot infer a supported review target from the active conversation
state
THE SYSTEM SHALL NOT expose commission review as an available tool

IF an unavailable commission review request is replayed from stale conversation
state
THE SYSTEM SHALL report that review is unavailable rather than performing a
best-effort review of an ambiguous target

**Rationale:** Reviewing the wrong code is worse than not reviewing. Hiding the
tool in unsupported contexts keeps agents on the safe path and prevents late,
ambiguous failures.

---

### REQ-CR-008: Keep Review Read-Only

WHEN commission review inspects repository state
THE SYSTEM SHALL NOT edit files, stage changes, commit, push, fetch remote data,
or move refs

**Rationale:** Review is an advisory action. Users commissioning review expect
observation and feedback only, not repository mutation or branch movement.

---

### REQ-CR-009: Honor Cancellation

WHILE commission review is collecting review material or waiting for LLM review
THE SYSTEM SHALL honor conversation cancellation
AND SHALL stop the review without returning fabricated findings

**Rationale:** Review can run longer than ordinary tools. Users need the same
ability to stop runaway or no-longer-needed review work that they have for other
long-running Phoenix operations.

---

### REQ-CR-010: Report Skipped Review Material

WHEN commission review excludes a changed file because its diff exceeds the
per-file or total review size cap
THE SYSTEM SHALL record the file and the cap it exceeded in a dedicated
`unreviewed` result, separate from advisory warnings

WHEN commission review excludes a binary, unsupported, or otherwise undiffable
changed file
THE SYSTEM SHALL include a warning identifying the skipped material and the
reason it was skipped

WHEN any changed file is excluded for any reason
THE SYSTEM SHALL NOT report the run as fully successful, and SHALL NOT silently
omit the file from the review result

**Rationale:** Users and agents need to know the limits of a review. A
size-driven coverage gap is a distinct, actionable fact — which files were too
large to review — so it is surfaced as its own typed result rather than buried
among advisory warnings, and it forces a non-success status so an incomplete
review can never be mistaken for a clean one.

---

### REQ-CR-011: Distinguish Partial Review Output From Failure

WHEN review execution is interrupted by model timeout or model transport failure
after Phoenix has parsed at least one finding or reviewer summary
THE SYSTEM SHALL return a partial review result that preserves the parsed output
AND SHALL NOT report the top-level status as failed

WHEN the same model interruption occurs before any finding or reviewer summary
has been parsed
THE SYSTEM SHALL return a failed review result with unavailable findings

WHEN the conversation is cancelled
THE SYSTEM SHALL rely on the runtime cancellation path rather than promising a
deliverable partial review result

**Rationale:** Failed means no actionable review output is available. Returning
populated findings with a failed top-level status makes callers choose between
ignoring useful feedback and trusting an apparently failed operation.

---

### REQ-CR-012: Report Review Stage Status

WHEN commission review returns a structured result
THE SYSTEM SHALL include typed status for target collection, diff collection, LLM
review, JSON parse or repair, and finding extraction

THE SYSTEM SHALL identify the stage where timeout, cancellation, parsing failure,
repair, truncation, or partial extraction occurred

**Rationale:** Callers need to know whether a result is incomplete because the
target could not be collected, the diff was truncated, the model failed, output
required repair, or findings were dropped during extraction.

---

### REQ-CR-013: Summarize Findings and Important Warnings

WHEN commission review returns findings
THE SYSTEM SHALL include deterministic finding counts by normalized severity and
total finding count after deduplication

WHEN commission review records operational warnings that affect trust or coverage
THE SYSTEM SHALL include a concise warning summary near the result status while
retaining detailed typed warnings

**Rationale:** Large finding sets should be scannable without each caller
recomputing severity counts. Important warnings such as parse repair, truncation,
and timeout must be visible where the result status is interpreted.

---

### REQ-CR-014: Keep User-Facing Results Free Of Token And Cost Metadata

WHEN commission review returns JSON or display data
THE SYSTEM SHALL NOT expose token counts, cost estimates, or cost objects in the
user-facing result contract

Internal accounting MAY retain LLM usage data when required by Phoenix tool
accounting, but that data SHALL NOT be serialized in the commission review result
or display payload.

**Rationale:** Commission review spend is governed by the approval gate and
required executive brief. Suspect or low-value token metadata in the result makes
operational trust worse rather than better.

---

### REQ-CR-015: Include Stable Finding Navigation Hints

WHEN the review model can identify a stable code symbol for a finding
THE SYSTEM SHALL preserve that symbol as an optional navigation hint in the
finding

THE SYSTEM SHALL treat file path as the required anchor; symbol and line are
supplemental hints and SHALL NOT replace the file anchor.

**Rationale:** Line numbers are useful but fragile across edits. Symbols make
findings easier to navigate while preserving the existing file-based contract.
