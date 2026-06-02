---
name: phoenix-extract-crate
description: The methodology for splitting a large crate into smaller ones — the decision principles and workflow that lead to a clean result, not one extraction's steps. Use when planning or executing a crate split, deciding what belongs in a shared base crate vs. what stays put, breaking a dependency cycle to enable a split, or sequencing any large incremental refactor where partial progress must stay shippable.
---

# Crate Extraction Methodology

How to split a monolithic crate into a layered, acyclic workspace without a
big-bang rewrite. This is the *reasoning*, not a checklist — the specifics of
any one extraction (which modules, what they measured) belong in its task file.

## 1. De-risk before you commit: spike, then decide

A large refactor's payoff is a hypothesis until measured. Don't commit to N
extractions on a projection. Extract the **single highest-payoff target**, wire
up the **minimal end-to-end** version of the win (not just the structural
change — the thing that actually delivers value), and **measure the real delta
against the baseline**. Then gate: if the delta matches the projection,
continue; if not, you've spent one unit of effort, not N, and you keep whatever
banked value the first step produced. The entry point to a big refactor is the
cheapest experiment that would falsify it.

## 2. Know whether your changes are a package deal

Before starting, ask what *each* part delivers **alone**. If the structural
change (the split) and the change that monetizes it (e.g. per-unit gating) each
deliver ~zero value in isolation and only pay off together, then budget for
**both or neither** — landing half and declaring victory ships cost with no
benefit. If a phase *does* have standalone value (cleaner boundaries,
compiler-enforced layering), that's a legitimate reason to do it — but say so
explicitly and justify it on *those* grounds, not on the headline metric it
doesn't yet move.

## 3. The layering rule: types sink, behavior floats

Design so cycles are unrepresentable. Establish one **acyclic base crate**
(here, `phoenix-core`) holding the shared, serializable *vocabulary* — the data
types and narrow service traits multiple crates speak in. Everything else
depends *downward* onto it and never sideways or up.

The decision rule when two crates need the same thing:

- **Pure data** (the type, its POD fields, derives) → **sinks** to the base.
- **Behavior** (parsing, I/O, anything pulling a heavy dep like `reqwest`,
  `axum`, `sqlx`) → **stays** in the crate that owns it, referencing the sunk
  type.

A type and its inherent impl can't live in different crates, so if an impl
needs a heavy dep, the type can't sink with that impl attached — split the impl
or reconsider. Never solve a cycle by dragging a heavy module *into* the base
crate "because everyone needs it": that pollutes the vocabulary layer with the
very deps it exists to stay free of.

## 4. Make the graph a DAG *first*, as separate steps

A module can only be lifted cleanly once it points only downward. Find the
wrong-way edges and fix each as its own validated commit *before* the
extraction — don't tangle cycle-breaking with the move. The edge tells you the
fix:

- **shared data type** referenced up/sideways → sink it (rule 3).
- **concrete dependency on a heavy service** → invert behind a **narrow trait**
  in the base; the heavy type impls the trait. (Depend on an interface you own,
  not a concretion you don't.)
- **misfiled glue** living in the wrong layer → relocate it *up* to the layer
  it actually belongs to; the genuine core stays low.
- **doc-link-only references** → not compile edges; downgrade them.

Extract bottom-up, so each new crate's dependencies are already crates or base.

## 5. Keep every step incremental and reversible

- **Move-down, re-export-up.** When you relocate code, re-export it at its
  original path so existing call sites resolve unchanged. This decouples the
  mechanical *move* from the optional *call-site rewrite* — two independently
  reviewable, independently revertible changes instead of one sprawling diff.
- **One logical move = one commit, each a green checkpoint.** Never batch
  extractions. A half-finished refactor then leaves a working, shippable tree
  at every commit — abandoning midway costs nothing structurally.

## 6. Trust only the real gate

A clean `cargo check` is not integration. A clean rebase with zero textual
conflicts is **not** integration — code that landed on the mainline while you
worked can be swept into a newly-extracted crate carrying paths that compiled
in the monolith but don't resolve across the new boundary, or needing deps the
new crate's manifest lacks. Before declaring done, run the **authoritative
end-to-end gate** (what CI runs — here `PHOENIX_CHECK_ALL=1 ./dev.py check`,
all lanes), not a proxy. Surface what it says honestly; a green proxy over a red
gate is worse than no signal.

## 7. Tighten at the new boundary, don't loosen

Promoting code across a new crate boundary turns private code into public API
and surfaces stricter lints (must-use, missing-`# Errors`/`# Panics` docs). That
warning is the new *contract* becoming visible — **satisfy it** (auto-fix where
the tool can; write the one-line doc where it can't). Reaching for `#[allow]` to
quiet it discards the boundary's value. The refactor should leave the code held
to a higher standard than it found it, never a weaker one.

## 8. Any work-skipping optimization must fail safe

If the split is paid for by an optimization that *narrows* work (lint/test only
the changed crate + its reverse-dependency closure), its correctness property is
**only ever narrow, never wrongly skip**. Every ambiguous input — an
unattributable path, a base-crate change, a tooling failure, a lockfile/codegen
touch — falls back to the **full** run. Escape hatches (a flag and an env var)
must force full in lockstep; a half-wired bypass that skips one dimension but
not another is a silent correctness hole.
