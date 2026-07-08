---
name: phoenix-ladle-fixture
description: Add or update a Phoenix UI Ladle fixture end-to-end, including scenario data, stories, QA capture scripts, dev.py qa wiring, tests, and fixture best practices.
---

# Phoenix Ladle Fixture

Use this skill when adding, changing, or reviewing a Phoenix UI fixture/story surface. A fixture is not complete when the story renders once; it should be reachable from Ladle, capturable via `./dev.py qa`, and backed by targeted tests or fixture data that make regressions obvious.

## Touchpoints checklist

For a new fixture surface named `<surface>`:

1. **Scenario data**
   - Put reusable scenario definitions under `ui/src/fixtures/<surface>/`.
   - Split concerns when useful:
     - `types.ts` for scenario IDs and shape.
     - `scenarios.ts` for concrete data.
     - `renderFixture.tsx` for the React fixture shell.
     - `index.ts` for exports.
   - Keep fixture data deterministic; avoid timers, random IDs, live network calls, or app-global state leaks.

2. **Rendered fixture marker**
   - Add a stable ready marker with the scenario id as its value, for example:
     ```tsx
     <main data-my-surface-fixture={scenario.id}>…</main>
     ```
   - The marker is the contract used by QA capture. Do not rely on visual text selectors or arbitrary sleeps.

3. **Ladle story**
   - Add `ui/src/stories/<surface>.stories.tsx`.
   - Prefer one story per scenario using the shared scenario list as the source of truth.
   - Set `storyName` to the scenario id so screenshots and story manifest keys stay readable.

4. **QA capture script**
   - Add `ui/scripts/capture-<surface>.mjs` using `runSurfaceCapture` from `capture-ladle-surface.mjs`:
     ```js
     import { runSurfaceCapture } from './capture-ladle-surface.mjs';

     runSurfaceCapture({
       surface: '<surface>',
       readyAttribute: 'data-my-surface-fixture',
       outDir: process.env.MY_SURFACE_QA_OUT ?? 'qa-artifacts/<surface>',
       viewport: { width: 960, height: 900 },
     });
     ```
   - Let the shared capture engine discover stories from Ladle's manifest; do not duplicate scenario lists in the capture script.

5. **Package script**
   - Add a `ui/package.json` script:
     ```json
     "qa:<surface>": "playwright install chromium && PHOENIX_LADLE=1 node scripts/capture-<surface>.mjs"
     ```

6. **`./dev.py qa` integration**
   - Add a `cmd_qa_<surface>()` wrapper near the other `cmd_qa_*` functions.
   - Add a subparser entry in the `qa` parser.
   - Add a dispatch branch in `args.command == "qa"`.
   - Verify `./dev.py qa --help` lists the new surface.

7. **Tests and specs**
   - Add/update targeted component or pure-derivation tests for the state being fixture-tested.
   - If the fixture represents a normative behavior, update the relevant spec before or alongside the code.
   - For typed fixture data, run `cd ui && pnpm exec tsc --noEmit` or `./dev.py check`.

8. **Validation**
   - Run the focused tests for the touched component/derivation.
   - Run `./dev.py qa <surface>` at least once when adding a QA capture route, unless environment constraints make browser capture impossible; note that explicitly.
   - Run `./dev.py check` before commit.

## Best practices

- **Single source of truth:** scenarios drive both Ladle stories and QA capture; the capture script should only name the surface and ready marker.
- **State coverage over decoration:** include states that exercise important decisions, especially loading/error/disabled/edge states.
- **Stable screenshots:** avoid animations, live clocks, layout affected by host data, or externally fetched assets.
- **No hidden async:** if the fixture simulates loaded state, pass loaded data directly. If it simulates loading, mark the fixture ready after the loading UI is intentionally visible.
- **Provider wrappers:** render with the same lightweight providers the component needs (`MemoryRouter`, theme, app contexts), but do not boot the whole app unless the surface requires it.
- **CSS ownership:** fixture-only CSS belongs with the owning fixture or existing fixture styles; component CSS remains with the component owner.
- **Accessibility and action semantics:** if fixture states show buttons/links, test the same roles/labels/classes the product relies on, not just screenshots.

## Example commands

```bash
cd ui && pnpm exec vitest run src/components/MySurface.test.tsx
cd ui && pnpm exec tsc --noEmit
./dev.py qa my-surface
./dev.py check
```
