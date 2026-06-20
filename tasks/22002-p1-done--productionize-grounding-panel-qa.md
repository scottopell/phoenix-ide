# Productionize grounding panel QA screenshots and seeded integration

## Goal

Turn the grounding-panel QA fixture introduced on `task-22001-redesign-conversation-grounding-side-panel` into a reusable, first-class Phoenix UI QA workflow.

The current grounding-panel branch adds a useful dev-only route and committed screenshot artifacts, but the fixture data is inline and not integrated with the broader dev seed/mock/testing ecosystem. This task productionizes that approach so future complex UI surfaces can use typed scenarios, visual stories, screenshot capture, and real seeded integration without ad-hoc one-off routes.

## Branch / stacking requirement

This task will be created from `main` by default, but it must stack on top of the remote grounding-panel redesign branch before implementation:

```bash
git fetch origin
git rebase --onto origin/task-22001-redesign-conversation-grounding-side-panel main
```

or otherwise create the implementation branch from:

```bash
origin/task-22001-redesign-conversation-grounding-side-panel
```

Do not start from plain `main` unless PR #340 has already landed. The code this task extends lives on that branch, including:

- `ui/src/pages/GroundingPanelFixturePage.tsx`
- `ui/src/components/GroundingPanel.tsx`
- `ui/src/components/GroundingPanel.css`
- `ui/src/components/groundingSummaries.ts`
- `docs/qa/grounding-panel.md`

## Product direction

Adopt a layered QA model:

1. **Vitest** covers logic and regression behavior.
2. **`dev.py seed`** creates real integration conversations/projects.
3. **Ladle** renders deterministic visual scenarios from typed fixture data.
4. **Screenshot capture tooling** regenerates review artifacts on demand.

The grounding panel remains the first productionized surface, but the pattern should be reusable for other complex UI surfaces.

## Scope

### 1. Extract typed grounding-panel fixture scenarios

Refactor the current inline fixture route into shared modules, for example:

```text
ui/src/fixtures/groundingPanel/
  scenarios.ts
  mockApi.ts
  renderFixture.tsx
  types.ts
```

The extracted fixture layer should define typed scenarios for at least:

- full dark / full light
- empty states
- error states
- collapsed rail with live/attention states
- narrow panel width
- selected skill detail
- selected task detail

The same data should be reusable by tests, Ladle stories, and any retained dev-only route.

### 2. Add Ladle visual stories

Add Ladle as a lightweight Vite-native visual scenario harness.

Create grounding-panel stories backed by the extracted fixture renderer, for example:

```text
ui/src/stories/grounding-panel.stories.tsx
```

Stories should cover the scenario matrix above and provide stable story URLs suitable for screenshot capture.

Prefer Ladle over Storybook for MVP because it is lighter, Vite-native, and sufficient for deterministic visual QA. Do not add Storybook unless a concrete blocker makes Ladle unsuitable.

### 3. Add screenshot capture command

Add a reproducible command that captures the grounding-panel Ladle scenario matrix.

Candidate command shape:

```bash
cd ui
pnpm qa:grounding-panel
```

or, preferably, a project-level wrapper:

```bash
./dev.py qa grounding-panel
```

The capture process should:

- start or connect to Ladle on a deterministic local port
- visit each grounding-panel story URL
- wait for a deterministic ready selector
- fail on unexpected browser console errors
- write generated screenshots to an ignored artifact directory, not committed PNG snapshots

### 4. Avoid committed PNG churn

Do **not** commit regenerated PNG screenshots as part of the normal workflow.

The MVP should focus on reproducible commands and docs. Generated screenshots should be either:

- local untracked artifacts, or
- CI/uploaded review artifacts in a future workflow

Update `.gitignore` or artifact paths as needed so screenshot generation does not create accidental binary churn.

The existing committed screenshots from the grounding-panel redesign PR may remain historical review artifacts, but the new productionized workflow should not require adding new PNGs for each change.

### 5. Add seeded Grounding Panel QA integration conversation

Extend `./dev.py seed` with a real seeded project/conversation that appears in normal Phoenix navigation.

The seeded fixture should exercise real app integration for:

- cwd/project grounding
- deep file tree and long names
- task discovery from a real `tasks/` directory
- statuses: ready, in-progress/current, blocked, brainstorming, done, wont-do
- priorities p0-p4
- linked conversation slug where feasible
- project/user/built-in-like skills discovery via real skill files
- branch/current-task naming conventions

This seeded conversation does not need to fake runtime-only state such as MCP OAuth failures or live work-scope resources. Those remain covered by Ladle visual scenarios.

Document clearly which coverage comes from seed versus Ladle.

### 6. Keep or replace the dev-only route intentionally

Decide whether `__qa/grounding-panel` remains.

Acceptable outcomes:

- keep it as a thin wrapper around the extracted fixture renderer, or
- remove it if Ladle fully replaces the review/debugging use case

Do not leave the route as a separate, divergent fixture implementation.

## Acceptance criteria

- Grounding-panel scenario data is no longer inline in `GroundingPanelFixturePage.tsx`; it lives in typed reusable fixture modules.
- Ladle is installed and has grounding-panel stories for the agreed scenario matrix.
- A command exists to regenerate grounding-panel screenshots from the Ladle stories.
- Generated screenshots are not committed by default and do not show up as accidental binary churn.
- `./dev.py seed` creates a real Grounding Panel QA conversation/project for integration review.
- Docs explain:
  - how to open Ladle stories
  - how to regenerate screenshots
  - how to find/use the seeded integration conversation
  - what Ladle covers versus what seed covers
- Existing grounding-panel UI behavior from PR #340 remains intact.
- The workflow is general enough that another complex surface could add scenarios by following the same pattern.
- Validation passes:
  - `./dev.py check --lanes tsc,ui-lint,vitest`
  - `./dev.py tasks validate`
  - any targeted seed validation needed for `dev.py seed` changes

## Non-goals for MVP

- Pixel-diff visual regression gating.
- CI enforcement of screenshot diffs.
- Full design-system documentation.
- Replacing Vitest or seeded dev conversations.
- Modeling all MCP/work-scope edge states through backend seed data.

## Follow-up opportunities

- CI job that uploads generated screenshots as PR artifacts.
- Pixel-diff review once the scenario/capture workflow is stable.
- Applying the same fixture/Ladle pattern to Process Inspector, Chain page work scope, task approval, and PR feedback surfaces.
