Child of task 92010.

# Replace or amend ADR-025 and reconcile the shared ADR chain

## Objective
Correct the ADR layer so it explains the settled lifecycle/ownership decisions without falsifying ADR history or freezing an unaccepted draft as accepted truth.

## Exact target artifact clusters
- `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md`
- `specs/adrs/README.md`
- any newly added replacement/superseding ADR file under `specs/adrs/`
- any spec files whose ADR references must be corrected because the accepted rationale moved

## Settled facts this task MUST encode
- ADR-025 is an **unaccepted draft direction to replace or amend before acceptance**, not a frozen accepted historical artifact to rewrite in place as though the decision were already settled forever.
- Do not rewrite an accepted historical ADR as if its original context never existed.
- If ADR-025 is superseded, replacement must preserve the ADR chain honestly: state context, decision, status, and consequences with accurate chronology.
- Product conversation owns Open/History lifecycle; `WorkScope` owns resources.
- Continuation keeps one product conversation and one attached `WorkScope` across durable rows.
- Use Git-backed vs chat-only; avoid “project conversation” in ADR rationale unless describing a rejected/legacy term explicitly.

## Required work contract
- Decide whether the correct move is to amend ADR-025 as draft material, replace it with a new ADR, or supersede it with a later numbered ADR.
- Update `specs/adrs/README.md` so numbering, titles, statuses, and dependency tree remain internally consistent.
- Ensure ADR prose explains why lifecycle belongs to product conversation while resource ownership belongs to `WorkScope`.
- Keep normative requirements language in requirements; ADRs explain rationale and tradeoffs only.
- Grep for stale ADR references after editing and fix broken backlinks in touched spec files.

## Out of scope
- No code changes.
- Do not treat ADRs as status tables for implementation completeness.

## Evidence required before marking done
Append a completion note to this task body with these headings:
- **Files changed** — exact ADR/index/reference paths
- **Decisions captured** — whether ADR-025 was amended, replaced, or superseded and why
- **Validation** — README/index/reference grep checks run
- **Review corrections** — history/numbering/backlink fixes made after review, or `None`
- **Commit** — commit hash that landed the work
