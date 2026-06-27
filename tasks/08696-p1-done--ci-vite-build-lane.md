Add a production-bundle (`vite build` / rolldown) lane to `./dev.py check` so unresolved-import breaks are caught in PR CI instead of at `prod deploy`.

## Motivation

A stray `import './TaskApprovalReader.css'` referencing a file that never existed merged green via PR #375 and only surfaced when `./dev.py prod deploy` ran `vite build`. PR CI did not catch it (fixed in #387, this task is the prevention).

Root cause: the check lanes in `_LANE_DEFS` (dev.py) have no lane that runs the real production bundler. The `tsc` lane runs only `tsc -b --noEmit`, which type-checks but never resolves CSS/asset import specifiers. Rolldown (`pnpm run build`) is the only thing that resolves them, and it runs solely in `release.yml` and `prod_build` — neither gates a PR. So any unresolved CSS/image/asset import sails through PR CI and breaks the production build.

## Scope

Add a `ui-build` check lane to `cmd_check` that runs the production bundle (`pnpm run build`, or `vite build` directly) and fails on bundler errors.

Considerations:
- Input-gate it to the `UI` input set (like `tsc`/`ui-lint`/`vitest`) so it only runs when `ui/` changes — keep gated PRs fast.
- It depends on generated TS (`ui/src/generated/`) and `node_modules`; sequence/parallelize accordingly. The existing build-graph node at dev.py ~4938 (`add("ui-build", "vite build", ...)`) already models these edges (codegen + ui-deps -> ui-build) and can guide the lane wiring.
- Register the lane in `_LANE_DEFS`, add its step list to `_GRAPH_LANE_STEPS`, and wire it into the CI matrix lane split in `.github/workflows/ci.yml` (currently `tsc,ui-lint,vitest,ast-grep,allium,spec-anchors,pkglock` on the UI runner).
- Mind CHECK_TIMEOUT: a cold `vite build` here was ~1.6s in the #387 fix verification, so it is cheap, but confirm it fits the lane budget on CI hardware.
- This produces a real `ui/dist/`; ensure it does not dirty the committed tree or interfere with the rust lane's frozen-tree assumption (see the lane_rust docstring re: RustEmbed tolerating an empty embed dir during check).

## Done when

- A UI-gated `ui-build` lane runs `pnpm run build` in `./dev.py check`.
- Reintroducing an unresolved import (e.g. a bogus CSS import) fails `./dev.py check` locally and in PR CI.
- CI matrix updated; `./dev.py check --all` and gated runs both green.
