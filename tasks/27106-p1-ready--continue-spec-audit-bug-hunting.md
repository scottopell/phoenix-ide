<!--
ID 27106 chosen as the next free slot above 27105.
Created without `taskmd new` since the binary isn't installed in this
env; run `./dev.py tasks fix` if reallocation needed.
-->

# Continue systematic spec-audit bug hunting (high-ROI 🚧 / ❌ rollup)

## Why this is worth doing

Spec executive tables are the project's `TODO` list — but they only get
read when somebody scans for them. PRs #64, #65 closed three executive-
table partials in one session (REQ-NOTIF-002, REQ-TPANEL-006,
REQ-TASKS-UI-001, REQ-TASKS-UI-007) for ~150 LOC of frontend changes
plus spec status updates. That's the high-ROI tier: the gap is named
in the spec, the fix is small, and `./dev.py check` proves the rest of
the system still passes.

This task is a fresh-context continuation of that bug-hunting style.
Goal: get to as many ✅s as possible for as little code as possible,
land each as its own PR, and write up anything ambiguous as a follow-up
task (the way 27105 was carved off when REQ-TPANEL-008 turned out to be
a three-way contradiction rather than a frontend-only slice).

## Starting inventory (current main, generated 2026-05-10)

```
$ for f in specs/*/executive.md; do
    n=$(grep -cE "(🚧 Partial|❌ Not Started)" "$f");
    [ "$n" -gt 0 ] && echo "$n $f";
  done | sort -rn

8 specs/credential-helper/executive.md
7 specs/notifications/executive.md
5 specs/viewer_slot/executive.md
5 specs/browser-tool/executive.md
2 specs/subagents/executive.md
2 specs/prose-feedback/executive.md
2 specs/agent-identity/executive.md
1 specs/terminal-panel/executive.md
1 specs/patch/executive.md
```

33 gap rows across 9 specs. Re-run that command at session start —
landed PRs change the picture. Some of these are 🚧 (close to done,
small fix possible), some are ❌ (not started, often needs design
work). Pick from the 🚧 tier first.

## Methodology (tried-and-true from PRs #63–#66)

### 1. Triage

Read each candidate's row in the executive table. The Notes column tells
you the gap. Three flavors emerge:

- **Tiny** ("rename this string", "route this prop through showError",
  "clear this state on navigation"): just do it. PR-per-fix or bundle
  closely related ones. Update the executive status row in the same PR.
- **Significant** ("needs a new endpoint", "coordinate with backend
  spec X"): write up as a `tasks/NNNNN-pX-ready--slug.md` and move on.
  See `tasks/27105` as the template — the body lays out the contradiction
  + two viable resolution paths so the next implementer doesn't have to
  re-derive them.
- **Genuinely ambiguous** (multiple readings, none obviously right):
  use `AskUserQuestion` before touching anything. Don't guess on
  architecturally-significant decisions.

### 2. Always check audit-specs

`./dev.py audit-specs` cross-validates that every `REQ-*` anchor in code
resolves to a canonical declaration in `specs/`. Run it first thing —
sometimes a "not started" REQ is referenced from code already (drift),
or a code anchor names a REQ that was never minted (the
`REQ-TPANEL-009` ghost found mid-session). Either way it's a real bug
the audit catches.

### 3. Each PR's deliverable

For each gap closed:

- The implementation change (or spec rewrite, for path-A resolutions
  like 27105's option A).
- The status row in `specs/<name>/executive.md` flipped from 🚧/❌ to ✅,
  with a one-line note pointing at the closing change.
- Any cross-spec references (e.g. `specs/notifications/executive.md`'s
  cross-references block called out the gap on the partner spec — they
  need updating too, or the audit will flag a stale anchor next session).
- `./dev.py check` clean (14/14).

### 4. PR sizing

One spec status change per PR is the right size. PR #64 bundled
REQ-NOTIF-002 + REQ-TPANEL-006 because they shared the same
`useToast.showError` plumbing, but that was the limit — anything larger
becomes hard for the reviewer (Codex / Copilot) to give comprehensive
feedback on. Don't bundle just because two unrelated REQs are both
small.

### 5. When you find a contradiction

Example: REQ-TPANEL-008 said "frontend distinguishes the 409"; code
silently reclaims; partner backend spec said "reject with 409". Three
artifacts disagreed. The right move was *not* to pick a side and
implement; it was to call `AskUserQuestion`, capture the resolution
options as a `tasks/` file, and stop. Mimic that. Spec audits are
discovery work, not implementation work — when discovery finds
something genuinely undecided, the deliverable is the decision frame,
not the code.

## Concrete suggested starting points

These are guesses — verify in the actual exec rows before committing:

- **`specs/notifications/`** — REQ-NOTIF-008 (catch-up on SSE reconnect)
  may be a small slice if there's already SSE-reconnect plumbing to
  hook into. Worth a 30-min investigation. REQ-NOTIF-006 (per-event
  toggles) is bigger but spec text is concrete.
- **`specs/credential-helper/`** — 8 gap rows; without reading them I
  can't tell which are tiny vs significant. Triage pass first.
- **`specs/viewer_slot/`** — recently restructured (PR #58 era); some
  of those rows may be partial because the implementation moved faster
  than the status table. Check git log on the file before assuming the
  gap is real.
- **`specs/agent-identity/`** — 2 rows; small enough to fully triage in
  a single read.

Don't take this list as gospel — the inventory above (regenerated at
session start) is what counts.

## Anti-patterns to avoid

- **Don't write speculative spec text.** If a feature isn't in the
  executive table because nobody has decided to build it yet, don't add
  it to score a `✅`. Audit closes existing gaps; it doesn't open new
  ones.
- **Don't bundle audit work into a feature PR.** It dilutes the review.
- **Don't skip `./dev.py check`** even on "obviously trivial" changes —
  the spec-anchors lane caught REQ-TPANEL-009 drift in PR #64 that
  would have shipped silently otherwise.
- **Don't push commits to multiple branches in one shell** without
  re-checking `git status` between them. The session that produced PRs
  #65 + #66 had to use `git stash` + `git checkout main` + new branch
  + `stash pop` to keep work scoped to the right branch — easy to mix
  up otherwise.

## Acceptance

This task is "done" when one of:

- The candidate inventory above is reduced by at least 5 closed gaps
  (5 ✅s landed on `main`).
- All remaining 🚧 / ❌ rows are either (a) closed, or (b) captured as
  follow-up tasks with concrete resolution options laid out (27105
  template).

If neither is reachable in a session, the next session can re-read this
task and continue — the inventory section gives them a starting point
that doesn't require a long context replay.

## Pointers

- PR #63 introduced the `spec anchors` check lane in `dev.py` — it's
  the systematic enforcement mechanism this task leans on.
- PR #64, #65 are the worked examples of the methodology.
- PR #66 (this PR's sibling, task 27105) is the worked example of
  "discovery found a contradiction; capture it; move on."
- `AGENTS.md` — read first, especially the **Issue Discovery Protocol**
  section. The "in-conversation TODO vs taskmd new" rule is the deciding
  heuristic for #2 in the methodology above.
