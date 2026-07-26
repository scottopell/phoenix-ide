Child of task 92010.

# Migrate lifecycle-related legacy design.md content into spEARS v2 homes

## Objective
Apply the spEARS v2 migration rules to lifecycle-related legacy `design.md` material so rationale, behavior, requirements, and status each land in the right artifact type before new authoring drifts across file classes.

## Exact target artifact clusters
Likely affected legacy source material to inspect first:
- `specs/bedrock/design.md`
- `specs/chains/design.md`
- `specs/conversation-retrieval/design.md`
- `specs/conversation-ui/design.md`
- `specs/projects/design.md`
- `specs/pr-association/design.md`
- `specs/work-actions-bar/design.md`
- `specs/work-lifecycle/design.md`
- `specs/work-scope-ui/design.md`
- any additional `design.md` file proven by grep to contain lifecycle/continuation/follow-up/WorkScope language

Destination artifact classes:
- `requirements.md` for timeless user need / REQ text
- `.allium` for precise behavior
- `specs/adrs/*.md` for rationale and decisions
- `executive.md` for status/current reality

## Settled facts this task MUST preserve during migration
- Product conversation identified by durable root owns Open/History lifecycle.
- Close conversation is the action; History is the resulting state; never migrate “Closed” as lifecycle truth.
- Context continuation stays one product conversation and one attached `WorkScope` across `continued_in_conv_id` rows.
- `WorkScope` owns resources; conversation lifecycle does not.
- `Continue here`, `Start in new conversation`, and follow-up remain distinct behaviors with the side-effect boundaries already settled.
- Use Git-backed vs chat-only; avoid migrating “project conversation” forward except as explicit legacy wording.

## Required work contract
- Follow spEARS v2 migration rules from repo guidance: requirements timeless, ADRs historical rationale, Allium precise behavior, executive current reality.
- Do not copy task IDs, PR references, status bullets, or resolved-question logs into timeless artifacts.
- For each migrated section, decide whether it should move, be rewritten, or stay temporarily as legacy source material with a documented reason.
- Update cross-references after migration so touched specs point to current v2 homes instead of stale design.md prose.

## Out of scope
- No code changes.
- Do not delete unrelated legacy `design.md` files wholesale.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact legacy sources and destination artifact paths
- **Decisions captured** — section-by-section migration calls and why
- **Validation** — grep/spec-shape checks run
- **Review corrections** — migration cleanup after review, or `None`
- **Commit** — commit hash that landed the work
