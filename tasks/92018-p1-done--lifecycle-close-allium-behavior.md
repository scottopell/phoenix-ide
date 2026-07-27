Child of task 92010.

# Author lifecycle, Close, and continuation behavior in Allium clusters

## Objective
Make the lifecycle behavior precise in Allium without inventing a second product model: Open/History lifecycle, Close as the action, continuation as same-conversation context boundary, and latest-row derivation from existing topology.

## Exact target artifact clusters
- the lifecycle/conversation Allium specs touched by task 92010, likely including:
  - `specs/bedrock/*.allium`
  - `specs/work-lifecycle/*.allium`
  - `specs/conversation-ui/*.allium`
- any directly dependent Allium files whose imports/helpers must change to keep the model coherent

## Settled facts this task MUST encode
- Product conversation lifecycle is **Open** or **History** only.
- **Close conversation** is the operation; **History** is the postcondition. Never model **Closed** as a third lifecycle state.
- Context continuation is one product conversation spanning multiple durable transcript rows linked by `continued_in_conv_id`.
- Continuation preserves the attached `WorkScope`; it does not create a new product lifecycle, provenance family, or fresh work ownership.
- The latest row is computed from continuation topology and durable rules; do not create duplicate “latest conversation” authority in Allium entities.
- `Continue here` resolves approved-task review and resumes the same context with no lifecycle, provenance, `WorkScope`, worktree, branch, or PR side effect.
- `Start in new conversation` creates a separate Open conversation with fresh `WorkScope`/worktree, exact approved task only, and visible **Derived from** provenance.

## Required work contract
- Trace each behavioral rule back to the settled requirements terminology from task 92017.
- Declare every helper used in `@guidance`, `requires`, `let`, or `ensures` blocks.
- Keep artifact-type boundaries clean: Allium defines behavior, not rationale or status.
- Make continuation/read-only predecessor behavior explicit without redefining History.
- Capture blocking review resolution semantics so `Continue here` is side-effect free beyond resuming the same conversation.

## Out of scope
- No requirements/ADR/executive rewrites except where another task explicitly owns them.
- No code changes.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact `.allium` paths
- **Decisions captured** — bullets for lifecycle/continuation/approval semantics encoded
- **Validation** — exact `allium check` commands and outputs summarized
- **Review corrections** — helper/import/cross-spec fixes made after review, or `None`
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

**Decisions captured**
- Parent task 92018 is complete through six child/spec-review tasks: `92025`, `92026`, `92027`, `92028`, `92029`, and `92030`.
- Review corrections across the child stack included: unified close authority on projects/spec surfaces, unified SSE lifecycle projection, WorkScope-vs-lifecycle cleanup boundary clarifications, approved-task placement exactness, and the final recovery acceptance removing the residual Continue-here mode transition from `specs/bedrock/bedrock.allium`.
- Final recovery acceptance also classified residual `mode` hits as internal execution authority only, and `requested_branch` in durable creation as generic legacy/internal creation API rather than the unified user-facing conversation path.
- Remaining implementation drift findings were explicitly deferred to implementation gates, not treated as spec blockers: executor Continue-here mode upgrade behavior and DB/task fresh-continuation behavior.

**Validation**
- See child task evidence, especially `tasks/92030-p1-in-progress--cross-file-allium-review.md`, for the final semantic grep set, individual+combined `allium check` runs, `python3 scripts/spec_shape_check.py`, and `./dev.py tasks validate`.
- This parent summary task did not introduce additional spec artifacts beyond status/evidence reconciliation.

**Review corrections**
- Summarized the six-child closure and final review corrections only; no new spec beyond the child-owned corrections.

**Commit**
- Pending local commit for task status/evidence reconciliation.
