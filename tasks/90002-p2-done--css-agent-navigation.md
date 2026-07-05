# Improve CSS navigability for agents

## Goal

Make the UI CSS easier for LLM agents and humans to navigate safely without attempting a large CSS architecture rewrite.

The current state is workable but high-friction: `ui/src/index.css` still owns most app styling, while some newer components already have colocated CSS. This task should establish clear ownership rules and demonstrate the preferred pattern through a few low-risk extractions.

## Preferred direction

Optimize first for **agent navigation**, not maximum line-count reduction or heavy enforcement tooling.

Use **small exemplar splits**:

1. Inventory the major sections of `ui/src/index.css` and identify their likely owner/surface.
2. Add a clear top-level map / ownership contract so future agents know where styles belong.
3. Extract 2–4 low-coupling sections from `index.css` into colocated component/page CSS files.
4. Run stylelint/build and targeted visual checks for touched surfaces where practical.

## Non-goals

- Do not migrate to CSS modules or another styling system.
- Do not attempt to fully decompose `index.css` in one pass.
- Do not add custom lint enforcement unless the need becomes obvious during implementation.
- Do not make broad visual redesign changes.

## Candidate extraction targets

Pick final targets during implementation based on import boundaries and risk, but prefer sections that are:

- visually self-contained,
- already owned by a clear React component/page,
- not relied on as shared primitives across multiple unrelated surfaces,
- unlikely to depend on fragile source-order specificity.

Possible candidates from the initial scan include sections such as:

- browser/profile structured response styles,
- chain page styles,
- auth strip / credential helper styles,
- file attachment composer styles,
- seed conversation breadcrumb styles,
- other clearly bounded late-file sections with obvious owning components.

Avoid extracting foundational/shared areas in this first pass, such as:

- theme variables,
- base reset/body layout,
- message markdown primitives,
- broad conversation list/message styles,
- mobile/global layout overrides unless they are directly tied to an extracted owner.

## Phases and done criteria

### Phase 1 — CSS inventory

Done when:

- Major `index.css` sections are mapped to one of:
  - global/base,
  - shared primitive,
  - app shell/layout,
  - page-owned,
  - component-owned,
  - legacy/unclear.
- The implementation notes identify 2–4 extraction targets and explain why they are low-risk.

### Phase 2 — Navigation contract

Done when:

- `index.css` has a concise top-of-file navigation map.
- The map states the default rule for new styles: colocate page/component-specific CSS with the owning TSX file; keep only global/base/shared primitives in `index.css`.
- The comment is practical navigation guidance, not a long design essay.

### Phase 3 — Exemplar extractions

Done when:

- 2–4 selected sections have been moved out of `index.css` into colocated CSS files.
- Owning components/pages import their CSS directly.
- Source order dependencies are preserved or made unnecessary.
- Extracted files use existing naming conventions and continue to pass stylelint.
- No duplicate semantic styling remains between `index.css` and the new files.

### Phase 4 — Verification

Done when:

- `./dev.py check` or the appropriate gated checks pass, including CSS linting.
- If full check is too expensive, at minimum run:
  - UI typecheck/build or the relevant `./dev.py` lane,
  - `pnpm lint:css` through the project-approved workflow,
  - targeted screenshot/manual verification for touched surfaces when practical.
- Any visual or specificity risk discovered during extraction is either fixed or explicitly deferred with a concrete follow-up note.

## Review focus

Reviewers should be able to answer:

- Can a future agent tell where to put a new style?
- Did the extracted CSS land beside an obvious owner?
- Did this reduce navigation ambiguity without creating broad churn?
- Are the remaining `index.css` sections easier to scan than before?

## Follow-up options

After this lands, possible future tasks include:

- Continue extracting page/component-owned sections opportunistically.
- Add lightweight tooling if `index.css` keeps growing with page-specific styles.
- Introduce a CSS ownership inventory doc only if the top-of-file map proves insufficient.
