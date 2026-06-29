# Traceability

spEARS is built so that one identifier — the `REQ-ID` — threads through every
artifact and every layer, and a single `grep` reconstructs the full story of a
requirement: what it is, where it stands, how it behaves, why it was built that
way, and which code implements it.

## The suture

Every requirement has an immutable `REQ-<ABBREV>-###`. That ID is the one thing
shared by all the artifacts, each of which describes the requirement from a
different angle:

```text
REQ-TA-001
├── requirements.md            the requirement itself (WHAT + user-why)
├── executive.md               its current status (WHERE we are)
├── specs/adrs/NNN_*.md        the decisions that shaped it (WHY) — via each ADR's Affects:
├── <feature>.allium           the rule that models its behavior (HOW), when warranted
├── src/…  // REQ-TA-001        the code that implements it
└── tests/… @requirement        the tests that verify it
```

Because the ID never changes (see [ears-guide.md](ears-guide.md)), these links
never go stale. Rename the requirement’s prose, refactor the code, supersede a
decision — the ID holds it all together regardless.

## Grep-based verification

The whole web is greppable.
To see everything about a requirement:

```bash
rg "REQ-TA-001"
```

To list every requirement in a feature:

```bash
rg "^### REQ-" specs/task-approval/requirements.md
```

To find every requirement comment in code:

```bash
rg "// REQ-" src/
```

An ID that appears only in `requirements.md` and `executive.md` is unimplemented
— fine, if its status says so.
An ID that appears in code but not in `requirements.md` is the definition of
over-engineering here: code with no requirement behind it.

## Why this stays cheap in spEARS

Cross-artifact agreement is the failure mode that eats review cycles in spec
systems that keep several prose descriptions of the same model — a
`requirements.md`, a `design.md`, an `executive.md`, and a behavioral spec all
naming the same fields and states, drifting apart the moment one is edited.

spEARS removes most of that surface by construction.
There is no `design.md` re-describing behavior, so there is no second prose
model to disagree with the first.
What remains is a narrow, ID-anchored agreement — a REQ-ID and its Allium rule,
an ADR and the REQ-IDs it `Affects:`, a status row and its requirement.
Each link is checkable by grepping one identifier, not by reading two documents
side by side and hoping they still match.
The suture is cheap precisely because there is only one of it.

## The matrix

For any requirement, the full traceability picture:

```text
REQ-TA-001: Approve a pending task
  ├── requirements.md          definition + rationale
  ├── executive.md             status: ✅ / 🔄 / ❌
  ├── specs/adrs/004_*.md      Affects: REQ-TA-001 — why approval is irreversible
  ├── task-approval.allium     rule ApproveTask — exact behavior (if specced)
  ├── src/approval.rs          // REQ-TA-001 — implementation
  ├── tests/approval_test.rs   @requirement REQ-TA-001 — verification
  └── git log --grep REQ-TA-001  the commits that touched it
```
