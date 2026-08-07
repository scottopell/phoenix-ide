Child of task 92010.

# Normalize lifecycle terminology and authority in requirements clusters

## Objective
Rewrite the lifecycle/authority baseline in the affected timeless requirements so every downstream spec task starts from the same settled product model and does not rediscover vocabulary.

## Exact target artifact clusters
- `specs/bedrock/requirements.md`
- `specs/projects/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/conversation-ui/requirements.md`
- `specs/conversation-retrieval/requirements.md`
- any other `requirements.md` files directly edited to remove conflicting lifecycle/authority terms discovered during grep

## Settled facts this task MUST encode
- Product **conversation** is identified by the durable root and owns the only user-facing lifecycle: **Open** or **History**.
- **Close conversation** is the action. **History** is the resulting state. Never define or imply **Closed** as a lifecycle label.
- Context continuation is a product context boundary implemented by multiple durable transcript rows linked by `continued_in_conv_id`; it remains one product conversation.
- The latest row is derived from `continued_in_conv_id` traversal and live-state rules; never introduce duplicate authority for “latest”.
- A conversation may have a `WorkScope` attached, but lifecycle belongs to the product conversation, not to `WorkScope` or a transcript row.
- Use **Git-backed** vs **chat-only** when needed. Do not introduce or preserve **project conversation** as the normative product noun.
- Legacy names may appear only as legacy compatibility/migration language.

## Required work contract
- Grep the target requirement clusters for conflicting terms before editing.
- Remove vague/generated guardrails such as “Closed lifecycle”, “project conversation”, or ambiguous “authority” claims that could create parallel truths.
- Make the root aggregate, continuation topology, and derived latest-row authority explicit in timeless language.
- If a requirement must mention `WorkScope`, state that it owns resources while conversations own lifecycle.
- Leave downstream behavior detail to Allium/ADR tasks; do not turn requirements into a changelog or design log.

## Out of scope
- No edits to `.allium`, ADRs, `executive.md`, code, or task status.
- No new product taxonomy beyond the settled facts above.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact `requirements.md` paths
- **Decisions captured** — bullets naming each terminology/authority correction made
- **Validation** — grep commands and any spec-shape checks run
- **Review corrections** — follow-up fixes made after self-review or peer review, or `None`
- **Commit** — commit hash that landed the work

## Completion evidence

**Files changed**
- `specs/bedrock/requirements.md`
- `specs/projects/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/chains/requirements.md`
- `specs/conversation-retrieval/requirements.md`
- `specs/pr-association/requirements.md`
- `specs/global-recall/requirements.md`
- `specs/conversation-creation/requirements.md`

**Decisions captured**
- Re-grounded lifecycle language on Open → History, with close action terminology replacing terminal/closed wording where this task touched requirements.
- Clarified that continuation topology (`continued_in_conv_id`) produces multiple execution rows for one product conversation, and that latest execution authority is derived from that topology rather than stored in a second owner field.
- Clarified that `WorkScope` owns runtime resources only; product lifecycle remains attached to the durable root conversation.
- Defined Continue here as an approval checkpoint with no extra lifecycle, environment, repository, or provenance side effects beyond the approved task commit.
- Defined Start in new conversation as a separate Open conversation derived from the source, with a fresh `WorkScope`/worktree and only the exact approved task as starting context.
- Re-grounded fork/follow-up language so follow-up work is separate and fresh rather than a continuation of the origin.
- Replaced normative “non-git” wording with Git-backed vs chat-only where this requirement cluster needed that distinction.
- Clarified that PR associations are observed WorkScope history, not lifecycle ownership, and that branches/PRs are observed targets rather than product-owned lifecycle units.
- Explicitly excluded the Coordinator from ordinary conversation lifecycle/WorkScope semantics.

**Validation**
- Read-first artifacts reviewed: `AGENTS.md`, `specs/AUTHORING.md`, `tasks/92010-p1-in-progress--conversation-lifecycle-spec-schema.md`, `tasks/92017-p1-in-progress--terminology-authority-requirements-basel.md`, commit `7de83e234`, and the requested requirement files.
- Grep audit run before edits:
  - `rg -n "Archive|Clean up|Abandon|Mark merged|Work mode|Branch mode|project conversation" specs --glob 'requirements.md'`
  - `rg -n "continued_in_conv_id|Close conversation|History state|project conversation|chat-only|Git-backed|Start in new conversation|Continue here|follow-up|Coordinator|Closed lifecycle|closed lifecycle|WorkScope" specs --glob 'requirements.md'`
- Validation commands to run after edits:
  - `./dev.py check --lanes spec-shape`
  - applicable markdown timelessness/shape spot-checks from `specs/AUTHORING.md`

**Review corrections**
- Removed an accidental duplicated approval trigger line in `specs/projects/requirements.md` during self-review.

**Commit**
- `61a10b91fe33b93de104d6b80a6406056b994ce2`


## Review correction round 2

**Files changed**
- `specs/projects/requirements.md`
- `specs/work-lifecycle/requirements.md`
- `specs/conversation-retrieval/requirements.md`
- `specs/api/requirements.md`
- `specs/agent-identity/requirements.md`

**Decisions captured**
- Replaced remaining normative Abandon / Mark as merged lifecycle language with explicit Close conversation semantics, keeping legacy mentions only as deprecated compatibility inputs.
- Removed ordinary creation requirements for Managed / Work / Branch lifecycle modes and branch selection; Git-backed creation now provisions one detached default-branch disposable worktree with no mode picker.
- Reframed write vs read-only distinctions as capability / authority rules rather than product lifecycle modes, while preserving the single-writer sub-agent rule.
- Replaced auto-archive requirements with explicit Close / History language.
- Reframed existing-branch work as repository activity inside the disposable worktree rather than as Branch mode conversation creation.
- Preserved chain transcript Q&A value by restating it as unified-conversation transcript Q&A on the normal conversation surface.
- Removed task-branch ownership side effects from task approval and Close semantics; branches and PRs are now repository facts Phoenix observes rather than lifecycle-owned artifacts Phoenix mutates.

**Validation**
- Semantic grep after corrections:
  - `rg -n '(Work mode|Branch mode|task branch|auto-archive|archiv(e|ed)|Abandon action|Mark as merged|mark-merged|Clean up|chain page|chain route|Explore mode|Managed mode)' specs/projects/requirements.md specs/work-lifecycle/requirements.md specs/api/requirements.md specs/agent-identity/requirements.md specs/conversation-retrieval/requirements.md`
  - Result after correction: only intentional legacy/deprecation matches remained:
    - `specs/api/requirements.md:142` — negative requirement forbidding legacy lifecycle writes
    - `specs/work-lifecycle/requirements.md:23` — deprecated compatibility note for legacy verbs
    - `specs/projects/requirements.md:423` — negative requirement forbidding automatic archive/auto-close
- AUTHORING timelessness spot-check:
  - `rg -n 'task [0-9]{3,}|tasks/[0-9]|PR #|see #[0-9]|RESOLVED [0-9]|Open Question|Q[0-9]\. |Progress:|Status Summary|✅|currently|for now|at the moment|recently|previously|landed|MVP|rollout|stopgap' specs/projects/requirements.md specs/work-lifecycle/requirements.md specs/api/requirements.md specs/agent-identity/requirements.md specs/conversation-retrieval/requirements.md`
  - Residual matches were unrelated pre-existing uses of `currently` in untouched API/spec text outside this task's semantic target.
- Required checks:
  - `./dev.py check --lanes spec-shape`
  - `./dev.py tasks validate`

**Review corrections**
- Independent review found that the first pass still left normative chain-page / Abandon / Mark merged / Archive / Work mode / Branch mode / task-branch / auto-archive requirements across the touched requirement cluster.
- This correction pass removed or deprecated those requirements in place, added explicit negative requirements where needed, and extended the edits to `specs/api/requirements.md` and `specs/agent-identity/requirements.md` so the grep-backed surface is consistent.

**Commit**
- `e9f888c9851e659aa27c80187ccce7c71ec8cbc8`


## Review correction round 3

**Files changed**
- `specs/agents/requirements.md`
- `specs/ask-user-question/requirements.md`
- `specs/bash/requirements.md`
- `specs/bedrock/requirements.md`
- `specs/chains/requirements.md`
- `specs/patch/requirements.md`
- `specs/subagents/requirements.md`
- `specs/tasks-ui/requirements.md`
- `specs/tmux-integration/requirements.md`
- `specs/wake-contracts/requirements.md`
- `specs/work-actions-bar/requirements.md`
- `specs/work-scope-ui/requirements.md`
- `specs/work-lifecycle/requirements.md`

**Decisions captured**
- Reclassified remaining repository-wide legacy-term hits into three buckets: rewritten contradictions, retained compatibility-only deprecations, and retained unrelated/internal terminology only when not part of the product taxonomy.
- Rewrote chain requirements away from a dedicated chain page and onto the unified conversation surface while preserving lineage Q&A, naming, and work-scope value.
- Reframed sub-agent, bash, patch, ask-user-question, and agent-capability requirements around execution authority / attached `WorkScope` / chat-only distinctions instead of product Work/Branch/Managed modes.
- Rewrote the work-actions matrix around REVIEW / RESOLVE / Close guidance, preserving PR-driven user guidance while removing Clean up / Abandon / Mark merged product actions.
- Removed normative task/conversation 1:1 ownership claims from tasks UI while keeping “current task” recognition and bidirectional navigation.
- Rewrote destructive wake/tmux lifecycle language to reference Close conversation and permanent Delete instead of Archive / abandon / mark-merged.
- Reworked bedrock approval, continuation, close, and execution-authority requirements to avoid product mode taxonomy and branch-lifecycle mutation, while preserving read-only planning, write-capable work-scope execution, and chat-only behavior.

**Retained matches and rationale**
- `specs/work-lifecycle/requirements.md:23` — retained intentionally as an explicit legacy compatibility statement for deprecated `Abandon` / `Mark as merged` inputs; it forbids them as current product actions rather than reintroducing them.

**Validation**
- Repository-wide sweep command reviewed and resolved:
  - `rg -n 'chain page|Abandon|Mark as merged|Archive|auto-archive|Work mode|Branch mode|Managed mode|Clean up|project conversation' specs --glob requirements.md`
  - Final result: only the intentional legacy compatibility note in `specs/work-lifecycle/requirements.md:23` remains.
- Additional semantic zero-unexplained-hit spot checks:
  - `rg -n 'chain page|Clean up|Abandon|Mark as merged|Archive|auto-archive|Work mode|Branch mode|Managed mode|project conversation' specs --glob requirements.md`
  - `rg -n 'Close conversation|History|chat-only|WorkScope|read-only planning authority|write capability' specs/{bedrock,chains,work-actions-bar,work-scope-ui,patch,bash,subagents,agents,ask-user-question,tasks-ui,tmux-integration,wake-contracts,work-lifecycle}/requirements.md`
- AUTHORING / required checks run successfully:
  - `./dev.py check --lanes spec-shape`
  - `./dev.py tasks validate`

**Review corrections**
- Independent review flagged that the reopened phase needed a repository-wide sweep beyond the earlier file-scoped corrections.
- This pass extended the edits across every current grep hit, fixed the last chain-page and terminal-verb references, and documented the one retained compatibility hit explicitly.

**Commit**
- `78ccded872f6abeb988f783c0f10d26144717ed6`
