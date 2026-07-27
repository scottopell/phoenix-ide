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


## Completion note
### Files changed
- `specs/projects/requirements.md`
- `specs/bedrock/requirements.md`
- `specs/conversation-retrieval/requirements.md`

### Decisions captured
- WorkScope authority language now states that WorkScope owns resources; conversations, transcript rows, and subordinate execution conversations only have a WorkScope attached.
- Continuation requirements now keep the same ProductConversation and the same attached WorkScope instead of describing row-to-row ownership transfer or a latest row owner.
- `propose_task` is now excluded from all Direct/chat-only conversations, including chat-only execution inside a Git repository.
- Approved-task provenance direction is explicit: the spawned/derived target conversation records a source relation of kind `approved_task` that points to the source conversation.
- History follow-up is specified as a fresh Open conversation with a fresh WorkScope, fresh detached worktree when Git-backed, a new user objective, no transcript injection, and a visible `follow_up` source relation to the source conversation.
- Permanent Delete is specified as aggregate-only, non-cascading to related conversations/branches/PRs, preserving tombstone-grade source identity for surviving links, reconciling/removing FTS/index projections, and remaining idempotent on retry.
- Retrieval requirements now name the typed source relation set (`approved_task` and `follow_up`) and require deleted-source outcomes to differ from absent-source behavior.

### Validation
- `./dev.py check --lanes spec-shape`
- `./dev.py tasks validate`
- `rg -n 'latest row owner|live owner of that `WorkScope` moves|transfer `WorkScope` ownership|Direct conversation inside a Git repository|from the source conversation to the new one|from the spawned conversation to the originating conversation' specs`
- `rg -n 'follow_up|Deleted source|tombstone-grade source|idempotent success|chat-only Direct conversation' specs/bedrock/requirements.md specs/conversation-retrieval/requirements.md specs/projects/requirements.md`

### Review corrections
- Corrected source-relation direction to say the target conversation records the source.
- Removed WorkScope authority wording that implied row ownership transfer during continuation.
- Removed Direct/chat-only `propose_task` eligibility inside Git repositories.

### Commit
- `0bc2b2e0b` — `docs: finish workscope provenance requirements task`
