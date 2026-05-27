# Spec Authoring Pre-Flight Checklist

This is a verification checklist for spec changes (both new specs and edits
to existing ones). Run through it before pushing.

It is targeted at the failure modes that have actually cost iteration
rounds on prior PRs — not a generic style guide. Each item is mechanical
and fast.

## Pre-flight checklist

### 1. `allium check` passes with zero errors

```bash
allium check specs/*/*.allium 2>&1 | grep '"severity":"error"' | wc -l
# Expected: 0
```

A change that doesn't parse is not a stylistic concern — it's broken.
Run this before every push. The lane is also gated in `./dev.py check`,
but locally validating is much faster than a CI roundtrip.

### 2. Every `path:line` citation is greppable

Most stale citations are caught by:

```bash
# Extract all citations from a spec file:
grep -nE '`[a-zA-Z_/\.-]+\.(rs|tsx?|ts|css):[0-9]+' specs/<your-spec>/*.md

# Then spot-check 3-5 by name:
grep -n '<symbol-the-line-claims-to-reference>' <path>
```

Citations naturally rot as the codebase moves. Once-correct line numbers
drift on every refactor. When in doubt, cite a **symbol** (function name,
struct name, identifier) instead of a line number — the symbol is stable,
the line number is not.

If a line number is essential for navigation, accept that it may drift
and re-verify it on every push that touches the area.

### 3. Every wire-shape claim matches actual Rust types

For each Rust type the spec references:

```bash
grep -n "pub struct <Name>\|^    <field>:" crates/phoenix-ide/src/<path>
```

Common drift points:
- `DateTime<Utc>` serializes as RFC3339 string, not `i64`. If a spec says
  `field: i64` for something sourced from a `DateTime<Utc>`, verify the
  serialization shape on the wire.
- `#[serde(flatten)]` on a struct puts the inner struct's fields at the
  enclosing JSON level — check what's actually at the top of the JSON.
- `Option<T>` skipped via `skip_serializing_if = "Option::is_none"`
  becomes `undefined` on the TS side, not `null`.

### 4. Every TS shape claim matches `ui/src/api.ts` (or generated types)

```bash
grep -n "type <Name>\|interface <Name>" ui/src/api.ts ui/src/generated/
```

Common drift points:
- Discriminated unions use `type:` as the discriminator, not `phase:` or
  `kind:` — verify the field name in the union before pattern-matching
  against it in spec prose.
- Generated types under `ui/src/generated/` are derived from ts-rs and
  are not hand-editable; the source of truth is the Rust side.

### 5. Every helper used in `@guidance` or rules is either declared or
   marked PLACEHOLDER

Allium has no formal helper-function declaration; helpers are bare
identifiers. The spec's "Helpers" block (see
`specs/working-phase-visibility/working-phase-visibility.allium` for an
example) is the single source of truth for their signatures. Any rule
that uses a name not in that block is using an undeclared helper.

Quick check:

```bash
# Extract identifiers used like function calls in your spec:
grep -oE '[a-z_]+\([a-z_]+' specs/<your-spec>/*.allium | sort -u
```

Cross-reference with the Helpers block; anything missing is either a
typo or an undeclared helper that needs a signature added.

### 6. All four spec files agree on every named field / state / event

This is the failure mode that ate the most rounds on PR #155. The four
files (`requirements.md`, `design.md`, `executive.md`, `<name>.allium`)
all describe the same model from different angles. When you change a
field name or state value in one, the others must follow.

Audit pattern:

```bash
# For every named entity, field, enum value, event, helper:
grep -n '<the-name>' specs/<your-spec>/*

# Look for discrepancies: same name with different shapes, or one
# document still using the old name.
```

Particularly load-bearing names to cross-check:

- Wire variant names (e.g. `LlmFirstByte`, `LlmAttempt`)
- Wire field names and their types
- Allium enum values referenced in rules
- Helper function names

### 7. Cross-spec whitelist audit

If your spec adds a new SSE wire variant or a new field that another
spec enumerates, that enumeration MUST be updated in the same change.

The canonical cross-spec checklist for SSE variants is at the top of
`specs/sse_wire/sse_wire.allium`'s "Cross-spec checklist" block (above
`EphemeralEventAppendedToReplayRing`). Follow it.

For non-SSE additions, search for the same name across `specs/`:

```bash
grep -rn '<new-name>' specs/
```

A whitelist that doesn't mention your new variant is a structural bug,
not a polish item.

### 8. `./dev.py tasks validate` passes

Spec changes often touch the corresponding task file (status transitions
via `taskmd status`, or filename renames). Validate:

```bash
./dev.py tasks validate
```

### 9. PR description matches the branch state

If your PR description claims a cross-spec import that the branch
intentionally doesn't have (because the sibling spec is in a follow-up
commit), update the description, NOT the spec. The bot reviewer treats
the PR description and the branch contents as cross-referencing each
other and flags the mismatch.

## Anti-patterns observed in past reviews

These come from real iteration cycles on PR #155 — included so they
don't recur:

- **Inventing a new wire field when an existing row field already has
  the value.** Check `db/schema.rs` and `runtime.rs` before adding a
  field to `wire.rs`. Reuse is correct-by-construction; parallel
  representations are a future divergence vector.
- **Using `i64` for a timestamp on the wire when the source field is
  `DateTime<Utc>`.** Serde's default for `DateTime<Utc>` is RFC3339
  string. Either commit to the string (and convert on the client) or
  define a custom serde wrapper.
- **Catch-all `EventSource` listeners.** Native EventSource has no
  wildcard for named events. Spec the listener registration list
  explicitly, and add a cross-spec checklist entry that new variants
  require new listener registrations.
- **`if/then/else` expressions in Allium `let` bindings.** Not in the v3
  grammar. Use a helper function (declared in the Helpers block) or
  split into sibling rules with disjoint preconditions.
- **`when:` patterns with struct destructuring** (`SseEventReceived(view,
  Message{message})`). Not supported. Use positional named parameters:
  `SseMessageReceived(view, message_role, message_seq)`.
- **Entity-instance assignment in `ensures:`** (`StateBarDisplay{view} =
  X`). Not supported. Use field assignment: `StateBarDisplay{view}.field
  = ...`.
- **Helper named in a rule but never defined.** Add to the Helpers
  block; the helper's signature and semantics live there.
- **Stale references in a status table to the previous wire shape.**
  When you change a wire shape decision (e.g. carrier for tool start
  times), grep for the old reference in `executive.md`'s status table
  and update it. The status table is the document most prone to drift.
- **Marking a requirement traceability claim that the model doesn't
  cover.** If REQ-WPV-008 is claimed traceable but isn't modelled as a
  rule, either add the rule or add a `deferred` entry naming the
  requirement and explaining why it's emergent rather than modelled.

## When to NOT use this checklist

This is for spec changes, not implementation changes. If you're writing
Rust or TS code that consumes a spec, the spec's own invariants are the
source of truth — run the tests, follow the spec, ignore this doc.

If you're writing a one-off task / scratch / experimental spec that
won't ship: skip it. The checklist exists to keep PR #155-class
iteration cycles from recurring on shipped specs.
