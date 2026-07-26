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
