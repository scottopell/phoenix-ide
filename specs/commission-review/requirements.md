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
repository with workspace changes
THE SYSTEM SHALL review the current workspace changes

**Rationale:** Agents should not need to rebuild Phoenix's knowledge of the
active task. Inferring the target avoids reviewing the wrong branch or omitting
workspace changes because of hand-written diff plumbing.

---

### REQ-CR-005: Prevent Accidental Dirty Worktree Reviews

WHILE the review target is a git-aware task or worktree with uncommitted changes
THE SYSTEM SHALL require `allow_dirty_working_tree` to be explicitly true before
reviewing those changes

IF the worktree is dirty and `allow_dirty_working_tree` is false
THE SYSTEM SHALL reject the request with an actionable explanation
AND SHALL NOT call the review LLM

**Rationale:** Dirty reviews can mix intentional task work with scratch edits,
local debug changes, or generated files. Explicit opt-in makes that ambiguity
visible before Phoenix spends review budget.

---

### REQ-CR-006: Show Dirty State in Review Scope

WHEN dirty worktree review is allowed
THE SYSTEM SHALL include the dirty state and explicit dirty-review opt-in in the
approval details and structured result

**Rationale:** Users need to know whether review includes uncommitted work. The
same fact must remain visible after the review so findings can be interpreted
against the correct workspace state.

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

WHEN commission review excludes a large, binary, unsupported, or truncated file
THE SYSTEM SHALL include a warning identifying the skipped material and the
reason it was skipped

THE SYSTEM SHALL NOT silently omit changed files from the review result

**Rationale:** Users and agents need to know the limits of a review. Explicit
warnings prevent over-trusting a review that did not examine all changed files.
