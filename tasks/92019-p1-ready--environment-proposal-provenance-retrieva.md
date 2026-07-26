Child of task 92010.

# Specify WorkScope ownership, approval placement, provenance, and retrieval requirements

## Objective
Settle the surrounding requirements that make the lifecycle model operational: `WorkScope` resource ownership, conversation attachment rules, approved-task placement semantics, follow-up freshness, and bounded retrieval across continuation/provenance boundaries.

## Exact target artifact clusters
- `specs/projects/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/conversation-retrieval/requirements.md`
- `specs/conversation-ui/requirements.md`
- other directly affected `requirements.md` files that speak about task approval, follow-up, provenance, or retrieval scope

## Settled facts this task MUST encode
- `WorkScope` owns resources.
- A conversation may have a `WorkScope` attached.
- A Git-backed user conversation ordinarily resolves to one attached `WorkScope`.
- Sub-agent/execution conversations may share that `WorkScope` without cleanup authority.
- Use **Git-backed** vs **chat-only**; avoid **project conversation** as the product term.
- `Continue here` resolves approved-task review and resumes the same context with no product lifecycle, provenance, `WorkScope`, worktree, branch, or PR side effect.
- `Start in new conversation` creates a separate Open conversation, fresh `WorkScope`/worktree, exact approved task only, and visible **Derived from** source.
- Follow-up is separate and fresh; it does not continue the old context. Branches and PRs may be observed from linked work, but are not owned by the conversation.
- Retrieval/search scope may span same-conversation continuation rows and typed source links only where requirements deliberately authorize it; provenance is not lifecycle.

## Required work contract
- Replace vague “environment/proposal/provenance/retrieval” language with concrete requirement statements tied to the settled facts above.
- Separate ownership from observation: `WorkScope`/resource ownership vs observed branches/PRs.
- Separate continuation from derivation/follow-up.
- State exact side-effect boundaries for `Continue here` and `Start in new conversation` so later Allium/code work cannot guess.
- Keep requirements timeless; no task IDs, status tables, or implementation diary prose.

## Out of scope
- No ADR editing except where another child task owns it.
- No executive-only status writing.
- No code changes.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact `requirements.md` paths
- **Decisions captured** — bullets for `WorkScope`, approval placement, provenance, follow-up, and retrieval corrections
- **Validation** — grep/spec-shape checks run
- **Review corrections** — wording fixes made after review, or `None`
- **Commit** — commit hash that landed the work
