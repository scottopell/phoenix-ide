# Conversation Widget Audit — Rubric

A 25-point scoring system applied to every widget rendered in the conversation thread.
Produces a comparable score per widget so we can triage which ones need work.

## Dimensions

Each dimension scored 1–5. Total out of 25.

### 1. Information density

Does the widget earn every pixel?
Status conveyed inline via symbols and color rather than prose.
Progressive disclosure — essentials visible, details on demand.

- **5** — every element earns its place, status and value collapse into one glance, no
  wasted rows
- **3** — readable but some elements are redundant or overly verbose
- **1** — prose where a symbol would do, wasted vertical space, labels restate what code
  already implies

Anchors: the project’s “Show status inline” principle; the pre-add test (what info,
already shown elsewhere, does user need it).

**Density means signal-per-pixel, not pixel-minimization.** A widget that shows the
information the user needs in the fewest pixels is dense.
A widget that hides or summarizes real information to save pixels is *not* dense — it
has shifted the cost from pixels to user clicks.
Hiding payload to shrink the widget does not earn a density point; verbose-but-honest
scores higher than terse-but-lossy.
See dimension 5 (Fidelity) — the two dimensions are complementary, not in tension.

### 2. State legibility

Can you tell success / failure / in-progress / truncated from 6 feet away without
reading text? Uses the `✓ + ✗ …` grammar consistently.
Failure is unambiguous.

- **5** — state readable at a glance from both badge *and* body (tinted output pane,
  colored leading line, etc.); color + symbol redundant so color-blind safe
- **3** — badge is correct but the body of the widget looks identical in success and
  failure; you can tell state from the corner chip but not from the main content area
- **1** — must read output to know if it succeeded, or failure and success look similar

**Badge vs body:** a `✓/✗` corner chip that works is the floor, not the ceiling.
A 5 means the *body* of the widget also signals state (e.g., red tint on the output
pane, an error banner inline, a different background for the leading line).
Relying solely on the badge stalls at 3 — readable but not legible, especially when
widgets stack.

### 3. Consistency with shared widget grammar

Does it slot into the same header row, status placement, expand affordance, typography,
and spacing as sibling widgets?
Predictability beats cleverness.

- **5** — structurally indistinguishable from neighbors except for content
- **3** — mostly matches but has minor off-grid choices (custom colors, different
  padding, novel icons)
- **1** — invents its own layout, its own palette, its own icons

**Omitted sections that are semantically appropriate don’t cost points.** A widget that
skips the output pane because the tool has no meaningful output (e.g., `think` — the
“output” is trivial confirmation text) is conforming to the grammar, not breaking it.
Penalize when a widget *deviates* from the shared scaffold (novel header shape,
different expand affordance, different icon family), not when it *elides* a section that
would be empty or noisy.

### 4. Scannability in context

Stress-tested in a long conversation, not in isolation.
When 20 of these stack, does the hierarchy survive?
Can you skim 50 messages and find “the bash that failed”?

- **5** — maintains rank in a crowded scroll; low-priority widgets recede, high-priority
  ones pop
- **3** — fine in isolation but contributes to visual noise when stacked
- **1** — every instance demands equal attention; stacks destroy the flow

Scannability is the biggest blind spot — isolated screenshots cannot score it.

### 5. Fidelity

Does the widget tell the truth about what happened?
Silent truncation, `...` with no count of what was dropped, hidden errors, summaries
masquerading as raw output — all are UI-layer data loss.

- **5** — if data is dropped or summarized, widget says so explicitly with a recovery
  path (expand, copy, raw view)
- **3** — truncation exists but the fact of truncation is visible (count shown, expand
  affordance works)
- **1** — silently hides data; “output” may not actually be the output

Ports the project’s correct-by-construction / “capability gaps are logged, not silenced”
principles to the UI layer.

## Scoring process

1. Capture screenshots per widget (see below).
2. Score each dimension 1–5 with a one-line note citing specific evidence.
3. Sum to total.
4. Sort widgets ascending by total — bottom scorers get attention first.

Keep per-dimension notes terse and specific.
Bad: `"state legibility could be improved"`. Good:
`"success badge duplicates the text 'success'"`.

## Screenshot guidance

Target 3 screenshots per widget, stored in `ui-audit/screenshots/<widget-slug>/`:

1. **`01-success.png`** — isolated success state.
   Serves density, consistency, fidelity.
2. **`02-alt.png`** — non-happy-path state (error / truncated / loading — whichever is
   most material for that widget).
   State legibility depends on this.
   A widget that looks clean on success can be unreadable when it errored.
3. **`03-in-context.png`** — widget surrounded by 5–10 other widgets in a real
   conversation. Scannability literally cannot be scored from isolation.

Some widgets won’t have a meaningful #2 (user-message, think).
Skip rather than fabricate.

If the audit balloons, tier it: triage-score all widgets with just `01-success.png`,
then the bottom third by total get the full 3-screenshot treatment for a deeper second
pass.

## Stages

- **Stage 1 (this folder):** widget inventory, rubric, per-widget placeholder eval
  files.
- **Stage 2:** capture screenshots, fill in each per-widget eval file, generate a ranked
  summary.
- **Stage 3 (implied):** prioritize fixes for bottom scorers.

## Files in this folder

- `RUBRIC.md` — this file.
- `INVENTORY.md` — the widget inventory with locations.
- `<slug>.md` — one per widget, pre-filled header, empty sections to complete in Stage
  2\.
- `screenshots/<slug>/` — image captures (created in Stage 2).
