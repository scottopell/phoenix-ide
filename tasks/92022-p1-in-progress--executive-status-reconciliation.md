Child of task 92010.

# Reconcile executive docs and product terms after lifecycle-spec changes

## Objective
Bring executive/status artifacts into alignment with the settled lifecycle model and product vocabulary after the normative requirements/Allium/ADR work lands.

## Exact target artifact clusters
- affected `specs/*/executive.md` files for lifecycle, conversation, retrieval, UI, projects, and work-scope topics
- likely starting points to inspect:
  - `specs/bedrock/executive.md`
  - `specs/conversation-retrieval/executive.md`
  - `specs/conversation-ui/executive.md`
  - `specs/projects/executive.md`
  - `specs/work-lifecycle/executive.md`
  - `specs/work-scope-ui/executive.md`
- any touched status table or verification notes that still reference old lifecycle/product terms

## Settled facts this task MUST reflect
- Product conversation lifecycle is Open/History.
- Close conversation is the action; History is the resulting state; executive docs must not casually reintroduce “Closed” as a lifecycle label.
- Context continuation remains one product conversation over multiple durable transcript rows linked by `continued_in_conv_id`.
- `WorkScope` owns resources; conversations may attach to one; lifecycle does not move to `WorkScope`.
- Use Git-backed vs chat-only; avoid “project conversation” except when describing legacy/current-compat behavior explicitly.
- Follow-up is separate and fresh; branches/PRs may be observed but are not owned.

## Required work contract
- Update current-reality/status prose to match the final normative decisions without restating them incorrectly.
- Keep executive artifacts in executive voice: implementation state, verification coverage, known gaps.
- Mark legacy behavior as current reality or compatibility where appropriate rather than letting it masquerade as normative design.
- Reconcile status tables/verification matrices with any renamed REQs, ADR references, or Allium rules touched by earlier tasks.

## Out of scope
- No new timeless requirements language except where another child task owns it.
- No code changes.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact `executive.md` paths
- **Decisions captured** — terminology/status/reality corrections made
- **Validation** — grep and spec-shape checks run
- **Review corrections** — table/reference cleanups after review, or `None`
- **Commit** — commit hash that landed the work
