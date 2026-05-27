# Spec authoring pre-flight discipline

## Problem

PR #155 (foundational spec for activity indicators, task 58002)
took 8 rounds of automated review to stabilise. The failures
fell into a small set of repeatable patterns — wire-shape
mismatches, Allium grammar bugs, undeclared helpers, cross-file
drift, stale citations, cross-spec whitelist gaps. None were
hard to detect; all were caught by bots after-the-fact instead
of being prevented at draft time.

This task distills the lessons into a pre-flight checklist that
future spec authors run before pushing. Decide whether to:

- (a) Land it as `specs/AUTHORING.md` (standalone reference doc)
- (b) Fold the content into `AGENTS.md`'s "Specifications"
  section (single source of truth, but mixes process with
  general project docs)
- (c) Both — short pointer in AGENTS.md, long form in
  specs/AUTHORING.md

Recommend (c): a one-liner pointer in AGENTS.md ("Before pushing
a spec change, run the pre-flight checklist in
`specs/AUTHORING.md`") with the substance in the dedicated file.

## Pre-flight checklist (content)

The checklist below was used on PR #155's late commits and
caught issues before push. Drop it into `specs/AUTHORING.md`
verbatim (or rephrase to fit AGENTS.md style).

### 1. `allium check` passes with zero errors

```bash
allium check specs/*/*.allium 2>&1 | grep '"severity":"error"' | wc -l
# Expected: 0
```

A change that doesn't parse is broken, not stylistically off.
Run locally before every push — CI rounds are slow.

### 2. Every `path:line` citation is greppable

```bash
# Extract citations from a spec file:
grep -nE '`[a-zA-Z_/\.-]+\.(rs|tsx?|ts|css):[0-9]+' specs/<your-spec>/*.md

# Spot-check 3-5 by name:
grep -n '<symbol>' <path>
```

Line numbers rot on every refactor. Cite **symbols** (function /
struct / identifier names) when possible — stable across moves.

### 3. Every wire-shape claim matches actual Rust types

```bash
grep -n "pub struct <Name>\|^    <field>:" crates/phoenix-ide/src/<path>
```

Common drift points:
- `DateTime<Utc>` serializes as RFC3339 string, not `i64`. If a
  spec says `field: i64` for something sourced from a
  `DateTime<Utc>`, verify the wire shape.
- `#[serde(flatten)]` on a struct puts inner fields at the
  enclosing JSON level — check what's actually at the top of
  the JSON, not what the type name suggests.
- `Option<T>` skipped via `skip_serializing_if =
  "Option::is_none"` becomes `undefined` on TS, not `null`.

### 4. Every TS shape claim matches `ui/src/api.ts` or generated

```bash
grep -n "type <Name>\|interface <Name>" ui/src/api.ts ui/src/generated/
```

Discriminated unions use `type:` as the discriminator (not
`phase:` / `kind:`). Generated types under `ui/src/generated/`
are derived from ts-rs; source of truth is the Rust side.

### 5. Every helper in `@guidance` or rules is declared

Allium has no formal helper-function declaration; helpers are
bare identifiers. The spec's Helpers block (see
`specs/working-phase-visibility/working-phase-visibility.allium`
for an example) is the single source of truth.

```bash
# Extract identifiers used like function calls:
grep -oE '[a-z_]+\([a-z_]+' specs/<your-spec>/*.allium | sort -u
```

Cross-reference with the Helpers block; anything missing is a
typo or an undeclared helper.

### 6. All four spec files agree on every named field / state / event

The failure mode that ate the most rounds. `requirements.md`,
`design.md`, `executive.md`, and `<name>.allium` describe the
same model from four angles. When you change a name in one, the
others must follow.

```bash
grep -n '<the-name>' specs/<your-spec>/*
# Look for discrepancies: same name with different shapes
```

Particularly load-bearing names:
- Wire variant names (e.g. `LlmFirstByte`)
- Wire field names + types
- Allium enum values referenced in rules
- Helper function names

### 7. Cross-spec whitelist audit

If your spec adds a new SSE wire variant or a field another spec
enumerates, that enumeration MUST be updated in the same change.

The canonical cross-spec checklist for SSE variants is at the top
of `specs/sse_wire/sse_wire.allium`'s
`EphemeralEventAppendedToReplayRing` rule. Follow it.

```bash
# For non-SSE additions:
grep -rn '<new-name>' specs/
```

### 8. `./dev.py tasks validate` passes

```bash
./dev.py tasks validate
```

### 9. PR description matches the branch state

If your PR description claims a cross-spec import that the
branch intentionally doesn't have (sibling spec in follow-up
commit), update the description, NOT the spec.

## Anti-patterns observed on PR #155

Add to AUTHORING.md so they don't recur:

- **Inventing a new wire field when an existing row field has the
  value.** Check `db/schema.rs` and `runtime.rs` before adding a
  field to `wire.rs`. Reuse is correct-by-construction; parallel
  representations are a future divergence vector.
- **`i64` for a timestamp on the wire when the source is
  `DateTime<Utc>`.** Serde's default is RFC3339 string. Either
  commit to the string (and convert on the client) or define a
  custom serde wrapper.
- **Catch-all `EventSource` listeners.** Native EventSource has
  no wildcard for named events. Spec the listener registration
  list explicitly.
- **`if/then/else` expressions in Allium `let` bindings.** Not in
  v3 grammar. Use a helper (declared in the Helpers block) or
  sibling rules with disjoint preconditions.
- **`when:` patterns with struct destructuring** (e.g.
  `SseEventReceived(view, Message{message})`). Not supported.
  Use positional named parameters:
  `SseMessageReceived(view, message_role, message_seq)`.
- **Entity-instance assignment in `ensures:`** (e.g.
  `StateBarDisplay{view} = X`). Not supported. Use field
  assignment: `StateBarDisplay{view}.field = ...`.
- **Helper named in a rule but never defined.** Add to the
  Helpers block.
- **Stale references in executive.md's status table to the
  previous wire shape.** When you change a wire-shape decision,
  grep for the old reference in `executive.md` and update.
  Status tables drift more than the prose.
- **Marking a requirement traceability claim the model doesn't
  cover.** Either add the rule, or add a `deferred` entry naming
  the requirement and explaining why it's emergent.
- **Bubble or counter not reusable across turns.** A singleton
  per-view entity with terminal `complete` / `removed` states
  can't be re-armed. Use a reusable `not_present` ground state.
- **Empty data on a typed SSE `ping` event.** axum's
  `Event::data` drops empty-data events; use `data("ping")` or
  any non-empty payload.

## What this task delivers

- `specs/AUTHORING.md` (standalone reference doc)
- One-liner pointer added to `AGENTS.md`'s spec section
- Move this task to `done` when both land

## Why P2

The discipline only pays off when the next spec is being drafted
(task 58003 — `llm-retry-visibility`). Land before that work
starts to avoid re-paying iteration costs.
