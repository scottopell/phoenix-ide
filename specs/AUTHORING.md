# Spec Authoring Pre-Flight Checklist

Before pushing a change that adds or modifies a spec under `specs/`,
run this checklist locally. The failure modes below cost rounds of
review iteration on PR #155 (8 rounds before stabilising). Catching
them at draft time is cheaper than catching them in CI or in bot
review.

The checklist is a tool, not a process — skip the irrelevant steps,
but be deliberate about it.

## 1. `allium check` passes with zero errors

```bash
set -o pipefail
for f in specs/*/*.allium; do
  errs=$(allium check "$f" 2>&1 \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print(sum(1 for x in d.get('diagnostics',[]) if x.get('severity')=='error'))")
  if [ "$errs" != "0" ]; then
    echo "$f: $errs errors"
    exit 1
  fi
done
echo "All specs parse with 0 errors."
```

A spec that doesn't parse is broken, not stylistically off. Run
locally before every push — CI rounds are slow.

**Why per-file and not `allium check specs/*/*.allium | grep | wc -l`?**
The naive pipeline silently reports 0 if `allium` is missing or exits
non-zero with non-JSON output — `grep` finds no matches and `wc -l`
prints `0`, defeating the gate. `set -o pipefail` plus the explicit
JSON parse forces a real failure when the tool didn't run. If you
don't have `python3` handy, substitute `jq '[.diagnostics[] |
select(.severity == "error")] | length'`.

`allium check` returns JSON; the loop above is the canonical
zero-errors check. Warnings (e.g. "Deferred specification should
include a location hint") are non-blocking but indicate stylistic
gaps worth fixing.

## 2. Every `path:line` citation is greppable

```bash
# Extract citations from a spec file:
grep -nE '`[a-zA-Z_/.-]+\.(rs|tsx?|ts|css):[0-9]+' specs/<your-spec>/*.md

# Spot-check 3-5 by name:
grep -n '<symbol>' <path>
```

Line numbers rot on every refactor. Cite **symbols** (function /
struct / identifier names) when possible — stable across moves.

If a citation does point to a line, verify it's still correct by
reading the file at that line. The previous spec author's "1408" is
not your "1408" after a six-month feature branch.

## 3. Every wire-shape claim matches actual Rust types

```bash
grep -n "pub struct <Name>\|^    <field>:" crates/phoenix-ide/src/<path>
```

Common drift points:

- `DateTime<Utc>` serializes as RFC3339 string, not `i64`. If a spec
  says `field: i64` for something sourced from a `DateTime<Utc>`,
  verify the wire shape.
- `#[serde(flatten)]` on a struct puts inner fields at the
  enclosing JSON level — check what's actually at the top of the
  JSON, not what the type name suggests.
- `Option<T>` skipped via `skip_serializing_if = "Option::is_none"`
  becomes `undefined` on TS, not `null`.

## 4. Every TS shape claim matches `ui/src/api.ts` or generated

```bash
grep -n "type <Name>\|interface <Name>" ui/src/api.ts ui/src/generated/
```

Discriminated unions use `type:` as the discriminator (not `phase:`
/ `kind:`). Generated types under `ui/src/generated/` are derived
from ts-rs; source of truth is the Rust side. Never hand-edit
generated files — their headers say so, and `./dev.py check` will
catch drift via `git diff --exit-code -- ui/src/generated/` after
the Rust tests.

## 5. Every helper in `@guidance` or rules is declared

Allium has no formal helper-function declaration; helpers are bare
identifiers. The spec's Helpers block (see
`specs/working-phase-visibility/working-phase-visibility.allium` for
an example) is the single source of truth for their signatures and
semantics.

```bash
# Extract identifiers used like function calls:
grep -oE '[a-z_]+\([a-z_]+' specs/<your-spec>/*.allium | sort -u
```

Cross-reference with the Helpers block; anything missing is a typo
or an undeclared helper.

## 6. All four spec files agree on every named field / state / event

The failure mode that ate the most rounds on PR #155.
`requirements.md`, `design.md`, `executive.md`, and `<name>.allium`
describe the same model from four angles. When you change a name in
one, the others must follow.

```bash
grep -n '<the-name>' specs/<your-spec>/*
# Look for discrepancies: same name with different shapes
```

Particularly load-bearing names:

- Wire variant names (e.g. `LlmFirstByte`, `LlmAttempt`)
- Wire field names + types
- Allium enum values referenced in rules
- Helper function names
- REQ-* identifiers (every REQ-ID should be referenced in the
  Allium rule that models it, and in the executive.md status table)

## 7. Cross-spec whitelist audit

If your spec adds a new SSE wire variant or a field another spec
enumerates, that enumeration MUST be updated in the same change.

The canonical cross-spec checklist for SSE variants is at the top
of `specs/sse_wire/sse_wire.allium`'s
`EphemeralEventAppendedToReplayRing` rule. Follow it.

```bash
# For non-SSE additions:
grep -rn '<new-name>' specs/
```

## 8. `./dev.py tasks validate` passes

```bash
./dev.py tasks validate
```

If the spec change is tied to a new task, validate the task file
follows the canonical filename pattern.

## 9. PR description matches the branch state

If your PR description claims a cross-spec import that the branch
intentionally doesn't have (sibling spec in follow-up commit),
update the description, NOT the spec. The spec stands on its own
parse; the PR description tracks current branch state.

---

## Anti-Patterns Observed on PR #155

These came up during the 8-round review cycle. Avoid them at draft
time:

- **Inventing a new wire field when an existing row field has the
  value.** Check `db/schema.rs` and `runtime.rs` before adding a
  field to `wire.rs`. Reuse is correct-by-construction; parallel
  representations are a future divergence vector. (Example:
  `state_updated_at` is already on `Conversation`; do not add an
  `entered_at`.)

- **`i64` for a timestamp on the wire when the source is
  `DateTime<Utc>`.** Serde's default is RFC3339 string. Either
  commit to the string (and convert on the client) or define a
  custom serde wrapper. Don't have one carrier use RFC3339 and
  another use `i64` for the same value — that's two parallel
  representations.

- **Catch-all `EventSource` listeners.** Native `EventSource` has
  no wildcard for named events. Spec the listener registration
  list explicitly. A forgotten registration silently drops events
  and degrades the heartbeat watchdog.

- **`if/then/else` expressions in Allium `let` bindings.** Not in
  v3 grammar. Use a helper (declared in the Helpers block) or
  sibling rules with disjoint preconditions.

- **`when:` patterns with struct destructuring** (e.g.
  `SseEventReceived(view, Message{message})`). Not supported. Use
  positional named parameters:
  `SseMessageReceived(view, message_role, message_seq)`.

- **Entity-instance assignment in `ensures:`** (e.g.
  `StateBarDisplay{view} = X`). Not supported. Use field
  assignment: `StateBarDisplay{view}.field = ...`.

- **Helper named in a rule but never defined.** Add to the
  Helpers block. The `grep -oE` check in step 5 catches this.

- **Stale references in executive.md's status table to the
  previous wire shape.** When you change a wire-shape decision,
  grep for the old reference in `executive.md` and update. Status
  tables drift more than the prose.

- **Marking a requirement traceability claim the model doesn't
  cover.** Either add the rule, or add a `deferred` entry naming
  the requirement and explaining why it's emergent.

- **Bubble or counter not reusable across turns.** A singleton
  per-view entity with terminal `complete` / `removed` states
  can't be re-armed. Use a reusable `not_present` ground state.

- **Empty data on a typed SSE `ping` event.** axum's `Event::data`
  drops empty-data events; use `data("ping")` or any non-empty
  payload.

- **Quoted strings in `requires:` when the field is an Allium
  enum.** `phase in { reconnecting, offline }` (unquoted), not
  `phase in { "reconnecting", "offline" }`. Quoted strings are
  correct when the field IS a String (e.g.
  `event_type in { "token", "state_change" }`).

- **Cross-spec entity references with mismatched actor types.**
  Allium actors are spec-local. Cross-spec lookups must go through
  shared scalar keys (typically `conversation_id: String`), the
  same pattern `conn/ClientConnection{conversation_id: ...}` uses.

---

## When to Skip the Checklist

The checklist is for spec-bearing PRs. For:

- Pure documentation edits to `AGENTS.md` or comments
- Test-only changes
- Code-only changes that don't touch `specs/`

...skip it. The point is to catch spec-shape failures, not to gate
every commit.

## Open Questions Are Mandatory

Per AGENTS.md "Resolving open questions is mandatory": an open
question in an Allium spec is not documentation. It is an
unresolved ambiguity that may hide a bug. Resolve every open
question — either as an explicit design decision in the spec, or
as a `deferred` entry with the rationale. Do not leave open
questions as prose notes or "future work."
