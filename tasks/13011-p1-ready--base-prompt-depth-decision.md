# Decide BASE_PROMPT depth: document the thin-base stance, or flesh it out

## The situation

`BASE_PROMPT` in `crates/phoenix-ide/src/system_prompt.rs` is three lines:
"You are a helpful AI assistant with access to tools … Use tools when
appropriate. Be concise … explain what you're doing briefly." A Phoenix-spawned
agent gets *only* that + whatever the project's `AGENTS.md` happens to say (via
the `<project_guidance>` injection) + a short mode block + the sub-agent suffix.

So out of the box a Phoenix agent has **no** guidance on: not over-engineering /
adding speculative abstractions / half-finished implementations; when to ask the
user vs proceed; handling ambiguity; surfacing failures rather than silently
working around them; executing hard-to-reverse / shared-state actions with care;
security posture (defensive/authorized work only). "The project's `AGENTS.md`
fills that" only holds for projects whose authors thought to write it — most
won't, and even this repo's `AGENTS.md` is mostly project-specifics, not a
general working-principles floor. For 1.0 that's a quality-and-safety floor
we'd be shipping without — hence p1: **the decision can't keep sliding.**

## The decision (this is a decision-required task)

Pick one — via `AskUserQuestion` if the implementer isn't already sure (the
repo owner has signalled they lean toward providing best-practices to flesh it
out, so Option B is the likely answer, pending their actual text):

**A — Own the thin-base stance, in writing.** Keep `BASE_PROMPT` minimal, but
make it an explicit, owned design decision: a short note in `specs/` (or, if no
spec covers system-prompt construction, a comment block in `system_prompt.rs`)
stating the harness deliberately ships a thin base and expects projects to layer
guidance via `AGENTS.md`. Trade: smallest/fastest prompt, zero opinions baked
in — but real agents on `AGENTS.md`-less projects fly blind on the basics above.

**B — Flesh out `BASE_PROMPT` with a tight "working principles" paragraph.** The
repo owner supplies the content (their best-practices for agents); the
implementer integrates it well — placement, ruthless trimming, no duplication.
Candidate themes (the owner's text is authoritative, this is just the shape):
don't over-engineer / no speculative abstractions / no half-finished work;
trust framework guarantees, validate only at boundaries; ask vs proceed for
ambiguous or risky/irreversible/shared-state actions; surface failures, don't
paper over them; defensive/authorized security posture only. Trade: every
Phoenix agent gets a sane floor — but it's opinionated and adds tokens to every
prompt, so it MUST stay tight (the "don't over-advise" lens that applied to
`AGENTS.md` applies doubly to `BASE_PROMPT`; a 200-word wall is worse than the
current 3 lines + nothing).

## Guardrails (apply to whichever path)

- `BASE_PROMPT` is the *floor* — it must not assume an `AGENTS.md` exists, and
  `<project_guidance>` (the project's `AGENTS.md`) layers on top of / overrides it.
- Whatever lands must not duplicate the mode blocks (worktree boundary,
  `propose_task` flow, "stay inside this worktree") or the sub-agent suffix.
- If Option B: keep the *added* guidance under ~150 words; update the
  `system_prompt` tests; don't let it grow into a second AGENTS.md.

## Out of scope

- Rewriting the mode blocks / sub-agent suffix.
- The `AGENTS.md` trim pass (separate; the owner reviewed `AGENTS.md` and was
  happy with it).
- Removing the "task ID prefix" line from the Work-mode block — already tracked
  in task 13009 (decouple task files from taskmd).

## Acceptance

- A decision is **recorded**: either (A) a `specs/` note or `system_prompt.rs`
  comment that explicitly owns the thin-base stance, or (B) `BASE_PROMPT`
  updated with the agreed working-principles text.
- If B: the added text is tight (< ~150 words), doesn't duplicate the mode
  blocks or `AGENTS.md`, and the `system_prompt` unit tests are updated to match.
- `./dev.py check` green.
