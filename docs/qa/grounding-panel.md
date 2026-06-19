# Conversation grounding panel QA

The side panel has a dev-only deterministic fixture at:

```text
/__qa/grounding-panel?scenario=full&theme=dark
```

Run Phoenix with `./dev.py up`, open the URL on the Vite dev server, and capture screenshots with the browser tooling. The route monkey-patches the fixture page's `fetch` calls only, so normal app data is untouched.

## Scenarios

- `full`: populated files, MCP, skills, tasks, and work-scope resources.
- `empty`: empty-state coverage for every async section.
- `errors`: API error/loading-state coverage for file/MCP/skills/tasks/work-scope calls.
- `collapsed`: collapsed rail with live work attention.
- `narrow`: minimum useful expanded width.
- `skill-detail`: opens a skill detail view after mount.
- `task-detail`: opens a task detail view after mount.

Append `theme=light` or `theme=dark` to capture both supported palettes.

Example capture matrix:

```bash
for theme in dark light; do
  for scenario in full empty errors collapsed narrow skill-detail task-detail; do
    open "http://localhost:<vite-port>/__qa/grounding-panel?scenario=${scenario}&theme=${theme}"
  done
done
```

Screenshot fixture docs and final screenshot artifacts are stored under `docs/qa/artifacts/grounding-panel/`. This branch commits the final fixture captures:

- `final/full-dark.png`
- `final/full-light.png`
- `final/collapsed-dark.png`
- `final/empty-dark.png`
- `final/errors-dark.png`
- `final/skill-detail-dark.png`
- `final/task-detail-dark.png`

Reviewers can regenerate or extend the matrix from the route above instead of relying on brittle binary snapshots for every scenario.

## UX findings addressed

- The old rail labels mixed words, symbols, and raw counts (`/`, `T`, `0`), so collapsed state did not communicate grounding sections.
- Section headers used parallel bespoke implementations, producing inconsistent counts, affordances, and empty states.
- Project grounding was labeled as only “Files”; the redesigned header ties the panel to cwd/project and branch.
- Attention states existed inside some sections but did not share a common visual treatment at section level.
- Task rows were pointer-clickable only; they now expose keyboard activation like skill rows.
