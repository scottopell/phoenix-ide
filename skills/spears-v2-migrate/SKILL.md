---
name: spears-v2-migrate
description: Phoenix-specific migration workflow for retiring legacy spEARS v1 design.md files into spEARS v2 artifacts. Use when migrating an existing specs/<name>/design.md, extracting ADRs, deciding whether behavior belongs in requirements or Allium, or removing stale design-doc references.
---

# spEARS v2 Migration for Phoenix Specs

Use this skill to migrate **one bounded Phoenix spec domain** from the legacy
v1 `design.md` artifact into spEARS v2 homes. This is a temporary migration
operator for Phoenix's mixed corpus; it is not the general authoring skill for
new specs.

## Target model

Phoenix's current spEARS v2 homes are:

- `requirements.md` — timeless user need and named requirements.
- `<name>.allium` — precise current behavior when the domain needs a formal layer.
- `specs/adrs/NNN_<slug>.md` — point-in-time design decisions and rationale.
- `executive.md` — status, current reality, and verification coverage.
- Code comments — implementation-local facts next to the symbol they explain.

A legacy `design.md` is not a current-design artifact. Do not keep a design doc
or stub only for history, posterity, or reassurance. If its unique content has a
v2 home or no living value, delete it.

## When to use

Use this skill when the task asks to:

- migrate or retire a legacy `specs/<name>/design.md`;
- extract ADRs from a design doc;
- decompose v1 spEARS artifacts into v2 homes;
- clean code/spec references that treat `design.md` as current authority;
- decide whether an existing design doc still contains unique content.

Do **not** use this skill for brand-new specs; use the normal `spears` guidance.

## Migration workflow

### 1. Read the whole domain before editing

Read these together:

- `specs/<name>/requirements.md`
- `specs/<name>/design.md`
- `specs/<name>/executive.md`
- every `specs/<name>/*.allium`
- `specs/adrs/README.md`
- code comments and generated docs that cite `specs/<name>/design.md`
- active ready/in-progress tasks that cite the design doc

Completed historical tasks may mention old design docs; do not rewrite task
history unless it is active guidance for future work.

### 2. Classify every load-bearing section

For each section or paragraph in `design.md`, assign exactly one home:

| Content type | Home |
| --- | --- |
| User need, product rule, named requirement | `requirements.md` |
| State, transition, invariant, precondition, lifecycle, partial failure behavior | `.allium` |
| Design decision, rejected alternatives, tradeoff rationale | `specs/adrs/NNN_<slug>.md` |
| Status, rollout state, verification coverage, current implementation note | `executive.md` |
| Symbol-local implementation fact | code/doc comment next to that symbol |
| Testing checklist, file layout, migration scaffold, stale rollout notes | delete unless a small verification note belongs in `executive.md` |

If the same behavior is already represented in requirements/Allium/executive,
record it as **already captured**. Do not duplicate it in an ADR.

### 3. Resolve ambiguities before moving text

When a section could fit multiple homes, decide by artifact semantics:

- Current behavior beats rationale: put behavior in requirements/Allium, not ADRs.
- Rationale beats comments: put durable tradeoffs in ADRs, not distributed comments.
- Local facts beat specs: keep low-level implementation facts near code symbols.
- Status beats timelessness: put progress/current reality in `executive.md`, not
  requirements or Allium.

If the ambiguity requires product/design judgment, present action-oriented
options to the user. Do not leave unresolved open questions in Allium or a
"legacy remainder" note when the next action is knowable.

### 4. Write ADRs only for real decisions

Create an ADR when the design doc contains a genuine fork:

- multiple viable options existed;
- the chosen option has tradeoffs;
- future maintainers need the reason, not just the rule.

Do not create ADRs for:

- behavior that requirements/Allium already specify;
- implementation details best explained by a local comment;
- migration history without ongoing design value;
- invented rationale that cannot be reconstructed from the artifacts.

After adding an ADR:

1. Use the next sequential number in `specs/adrs/`.
2. Fill every template section, including negative consequences.
3. Add it to `specs/adrs/README.md` quick reference.
4. Add a routing row if future tasks should consult it.
5. Update the dependency graph.

### 5. Remove stale authority references

Search for design-doc references before deletion:

```bash
rg "specs/<name>/design\.md|<name>/design\.md|design\.md" specs/<name> crates ui tasks AGENTS.md
```

For active references, replace with the right authority:

- REQ ID for requirement authority.
- Allium entity/rule/transition for precise behavior.
- ADR number for rationale.
- Symbol-local comment for implementation facts.
- Active follow-up task only when migration is genuinely blocked.

Do not leave comments that say "see design.md" after the file is removed.

### 6. Delete or explicitly block `design.md`

The desired end state is deletion.

Delete `specs/<name>/design.md` when:

- every unique behavior is in requirements or Allium;
- every unique rationale is in ADRs;
- every status/verification fact is in executive;
- implementation-local facts moved next to code;
- remaining migration/testing/file-layout prose has no living spec value.

Keep `design.md` only when migration is blocked by a named, unresolved decision
or missing formal behavior layer. If you keep it, add a short task-scoped note
that lists the exact blocker and create a follow-up task. Do not keep it for
posterity.

### 7. Validate

Run at least:

```bash
./dev.py check --lanes spec-shape,allium,spec-anchors,fast
```

Run full check when code, generated files, or broad repo guidance changed:

```bash
./dev.py check
```

Also run targeted Allium validation if you changed one formal spec and need
fast feedback:

```bash
allium check specs/<name>/<name>.allium
```

## Completion checklist

Before handing off:

- [ ] Unique design-doc content is classified and moved, verified captured, or deleted.
- [ ] ADRs are indexed and pass `spec-shape`.
- [ ] Requirements and Allium do not contain decision logs or rollout history.
- [ ] Active code/spec/task references no longer treat removed `design.md` as authority.
- [ ] `design.md` is deleted, or a concrete blocker and follow-up task exist.
- [ ] Relevant checks passed and are reported.

## Example outcome

A successful migration may end with:

- `specs/<name>/design.md` deleted.
- `specs/adrs/NNN_<slug>.md` added for durable tradeoffs.
- `.allium` updated for lifecycle or invariant gaps.
- active comments changed from "see design.md" to REQ IDs, Allium rules, or ADRs.
- follow-up migration tasks narrowed because this domain no longer belongs in the
  legacy design-doc inventory.
