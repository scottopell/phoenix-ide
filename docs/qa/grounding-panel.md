# Conversation grounding panel QA

The grounding panel uses a layered QA workflow:

1. Vitest covers logic and regression behavior.
2. `./dev.py seed` creates a real integration conversation backed by a real Git worktree.
3. Ladle renders deterministic visual scenarios from typed fixture data in `ui/src/fixtures/groundingPanel/`.
4. Screenshot tooling regenerates local review artifacts on demand.

## Ladle visual stories

Run the Vite-native story harness from the UI package:

```bash
cd ui
pnpm ladle
```

Then open the stable story URLs on port `61123`, for example:

```text
http://127.0.0.1:61123/?story=grounding-panel--full-dark
http://127.0.0.1:61123/?story=grounding-panel--skill-detail-dark
```

The stories are backed by shared typed scenarios, not page-local ad-hoc data:

```text
ui/src/fixtures/groundingPanel/
  scenarios.ts
  mockApi.ts
  renderFixture.tsx
  types.ts
ui/src/stories/grounding-panel.stories.tsx
```

Covered scenarios:

- `full-dark`
- `full-light`
- `empty-dark`
- `errors-dark`
- `collapsed-dark`
- `narrow-dark`
- `skill-detail-dark`
- `task-detail-dark`

The retained dev route is intentionally only a thin wrapper around the same renderer:

```text
/__qa/grounding-panel?scenario=full-dark
/__qa/grounding-panel?scenario=full&theme=light
```

## Regenerating screenshots

Use the project-level wrapper:

```bash
./dev.py qa grounding-panel
```

or run the UI script directly:

```bash
cd ui
pnpm qa:grounding-panel
```

The command ensures Playwright's Chromium browser is installed, starts Ladle on a deterministic local port, visits each grounding-panel story, waits for the fixture-ready selector, fails on browser console errors, and writes PNGs to:

```text
ui/qa-artifacts/grounding-panel/
```

That directory is ignored by git. Do not commit regenerated PNGs during normal review; upload or share them as local/CI artifacts when needed.

Historical screenshots from the redesign PR may remain under `docs/qa/artifacts/grounding-panel/`, but the productionized workflow no longer depends on committed binary churn.

## Seeded integration conversation

`./dev.py seed` creates a real seeded project and conversation named:

```text
Fixture Grounding Panel QA
slug: fixture-grounding-panel-qa
```

The fixture worktree lives under the worktree-local Phoenix data directory:

```text
.phoenix/seed-worktrees/grounding-panel-qa
```

Open Phoenix after seeding and select the conversation from normal navigation. It exercises real backend integration for:

- cwd/project grounding
- deep file tree and long file names
- task discovery from a real `tasks/` directory
- task statuses: ready, in-progress/current, blocked, brainstorming, done, wont-do
- task priorities p0-p4
- current-task conversation slug linking through Work-mode task metadata
- project, user, built-in-like, and child-project skill discovery through real `SKILL.md` files
- branch/current-task naming conventions

Seed data intentionally does not fake runtime-only state such as MCP OAuth failures or live work-scope resources. Those visual edge cases stay in Ladle where they can be deterministic.

## Coverage split

| Layer | Covers | Does not cover |
| --- | --- | --- |
| Vitest | logic, reducers, and regression assertions | visual layout matrices |
| Ladle | deterministic visual states, error/empty/collapsed/detail/narrow variants, light/dark | real backend discovery and navigation |
| `./dev.py seed` | real DB rows, real project cwd, real tasks/skills/file tree integration | synthetic MCP failures and live runtime work-scope edge states |

## UX findings addressed by the redesign

- The old rail labels mixed words, symbols, and raw counts (`/`, `T`, `0`), so collapsed state did not communicate grounding sections.
- Section headers used parallel bespoke implementations, producing inconsistent counts, affordances, and empty states.
- Project grounding was labeled as only “Files”; the redesigned header ties the panel to cwd/project and branch.
- Attention states existed inside some sections but did not share a common visual treatment at section level.
- Task rows were pointer-clickable only; they now expose keyboard activation like skill rows.
