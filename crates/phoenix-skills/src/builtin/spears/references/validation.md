# Validating the Markdown Layer

This guide is for checking a spEARS spec before it merges — catching the drift
and shape failures that otherwise cost rounds of review.
It validates the markdown layer (`requirements.md`, `executive.md`, `adrs/`);
for a feature carrying a `.allium`, the behavioral checks (`allium check`,
`weed`) are allium’s job and run only when allium is present.

**This is a tool, not a process.** Skip the checks that do not apply to your
change, but skip them deliberately.
The point is to catch spec-shape failures at draft time — far cheaper than
catching them in CI, in bot review, or six months later when a confidently wrong
spec misleads someone.

**A note on why this is short.** Much of what other spec systems must police,
spEARS removes by construction.
The single most expensive drift class — keeping multiple prose descriptions of
the same model in agreement — does not exist here, because there is no
`design.md` re-describing behavior alongside the requirements and the Allium.
Validation leans on *structure*, not vigilance.
What remains is genuinely irreducible.

## The checks

### 1. The spec reads as timeless — and the check knows which files are exempt

Every artifact *except* `executive.md` and `adrs/` must read as a standing
description of the system as if it had always been this way — no changelog
language, no status, no “we recently switched.”
[authoring.md](authoring.md) lists the banned phrases.

The crucial subtlety, and where a naive grep gets it wrong: the timeless rule is
**artifact-aware**.

- **Apply it to** `requirements.md` (and the prose portions of a `.allium`, per
  allium’s own rules).
- **Exempt** `executive.md` — status and the Open Questions & Future Directions
  section are *supposed* to be time-bound; that is the document’s whole job.
- **Skip `adrs/` entirely** — ADRs are deliberately dated, decision-bearing, “we
  chose X over Y.” The exact language the timeless check hunts for is correct
  and required there.

A flat “specs are timeless” grep that flags `✅` in `executive.md` or a date in
an ADR is not finding drift; it is misunderstanding the model.
Scope the check to the files that owe timelessness.

### 2. Every REQ-ID agrees across the artifacts that mention it

The `REQ-ID` is the suture (see [traceability.md](traceability.md)). For each
requirement, confirm it appears — with the same identity — everywhere it should:
defined in `requirements.md`, tracked in `executive.md`’s status table,
referenced in the Allium rule that models it (if the feature has one), and
carried in the code/test comments that implement it.
A REQ-ID that exists in `requirements.md` but appears nowhere else is either
unimplemented (fine — its status says so) or orphaned (a problem).

### 3. Citations use symbol names, not line numbers

Anywhere a spec or ADR points at code, it should name a symbol
(`SseBroadcaster::send_seq`), not a line (`runtime.rs:529`). Line numbers rot on
the next refactor; symbols survive moves.
Spot-check a few citations by grepping the symbol and confirming it still
exists.

### 4. Each kind of “unresolved” is in its right place

spEARS has no single open-questions list (see [authoring.md](authoring.md)).
Validate that each unresolved thing is where it resolves, not parked in the
wrong artifact:

- **No two requirements contradict each other.** This is the highest-value check
  — a contradiction that merges is a latent bug.
  If two requirements cannot both hold, one must win (often via an ADR) or be
  deprecated before merge.
- **No undecided behavior left vague in `requirements.md`.** A requirement whose
  meaning is genuinely undecided is not done — decide it, raise a `Proposed`
  ADR, or (with allium) an `open question`. `executive.md`’s Open Questions &
  Future Directions section is for forward-looking unknowns only, and may be
  non-empty at “done.”

### 5. Documents stay separated

No status or implementation detail in `requirements.md`; no code blocks in
`executive.md`; no decision logs anywhere but `adrs/`.
[authoring.md](authoring.md) has the full table.

### 6. The Allium layer — only when a `.allium` is present

If, and only if, the feature carries a `.allium`, run allium’s own checks via
the allium skill: `allium check` reports zero errors, `weed` reports no
spec↔code divergence, and no Allium `open question` declarations remain
unresolved. spEARS does not reimplement these — they are the behavioral layer’s
contract, and they simply do not apply to a spec that has no `.allium`.

## Write check scripts to fail loudly

Several of these checks are deterministic and worth scripting (we add them to
`scripts/` deliberately, one at a time).
When you do, heed the lesson that cost real review rounds: a gate that passes
silently when the tool is missing is worse than no gate.
A naive `some-check | grep | wc -l` prints `0` — a passing result — when the
command errored or the tool was absent.
Use `set -o pipefail`, parse real output, and make the script exit non-zero when
it could not actually run.
A green check must mean “the check ran and passed,” never “the check failed to
run.”

## When to skip

This is for spec-bearing changes.
For pure code changes that do not touch the spec, documentation-only edits, or
test-only changes, skip it — the goal is to catch spec-shape failures, not to
gate every commit.
