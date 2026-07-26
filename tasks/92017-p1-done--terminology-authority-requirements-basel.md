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
