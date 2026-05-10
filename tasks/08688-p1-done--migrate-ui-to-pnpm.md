Migrate `ui/` from npm to pnpm and harden `./dev.py` so the wrong package
manager (or wrong pnpm version) cannot be used by accident. Goal: end the
recurring lockfile-drift commits (e.g. 6c66c7a, 146452f) caused by multiple
npm versions across darwin laptop + linux remote hosts (sopell3 etc.) writing
to the same `package-lock.json`.

# Why pnpm

Diagnosis: `packageManager: "npm@11.12.1"` is decorative on hosts where
Corepack isn't enabled (sopell3 runs npm 11.6.1 even though the field says
11.12.1). Different npm versions resolve transitive optional/peer deps for
rolldown's ~15 platform-specific bindings differently, producing partial
locks that pass `npm install` but fail `npm ci`. pnpm's `pnpm-lock.yaml` is
deterministic across platforms by design, with explicit per-platform
resolutions for optional binaries — the entire failure mode goes away.

User has chosen pnpm. User wants to stay on rolldown (cutting edge is OK);
only the lockfile drift must go.

# Scope

## 1. Lockfile migration

- Run `pnpm import` in `ui/` to convert `package-lock.json` -> `pnpm-lock.yaml`.
- Delete `package-lock.json` and `ui/.npmrc` (engine-strict was an npm workaround).
- Add a minimal `ui/.npmrc` or `ui/.pnpmrc` only if pnpm needs config (e.g.
  `node-linker=hoisted` if vite/rollup plugins choke on pnpm's symlinked
  `node_modules` — verify both with hoisted and isolated layouts; prefer
  isolated unless something breaks).
- Update `ui/package.json`:
  - `packageManager: "pnpm@<latest 9.x>"` (verify exact version with `pnpm -v`
    after `corepack prepare pnpm@latest --activate`)
  - `engines`: drop the npm pin; keep node `>=22`. Optionally add `pnpm: ">=9"`.
- Verify `pnpm install --frozen-lockfile` succeeds on darwin-arm64 AND on
  one linux remote (sopell3) before committing.

## 2. dev.py pit-of-success enforcement

The existing npm call sites (grep for `npm`):
  - L364:  `npm install` (the engine-strict bypass hack — DELETE the bypass)
  - L513:  `npm run dev` (Vite)
  - L1421: `npm run lint`
  - L1684: `npm ci` (prod build worktree)
  - L1691: `npm run build`

Replace each with the pnpm equivalent invoked via Corepack:
  - `pnpm install` (lazy install path, only if `node_modules/.modules.yaml` missing)
  - `pnpm run dev`
  - `pnpm run lint`
  - `pnpm install --frozen-lockfile` (replaces `npm ci`)
  - `pnpm run build`

Add a `_ensure_corepack_pnpm()` helper that runs once at dev.py startup
(before any pnpm invocation):
  1. `corepack --version` — fail loudly if missing (suggest `npm i -g corepack`
     or upgrading node).
  2. `corepack enable` — idempotent; ensures shims are on PATH.
  3. `corepack prepare pnpm@<pinned> --activate` — installs/pins the exact
     pnpm version from `package.json#packageManager`. Read the pinned version
     from `ui/package.json` rather than hardcoding it in dev.py — single
     source of truth.
  4. Verify `pnpm --version` matches the pinned version; hard fail otherwise
     with a clear message ("expected pnpm X.Y.Z, got A.B.C — run
     `corepack prepare pnpm@X.Y.Z --activate`").

This replaces the `engine-strict` bypass at L357-364 — Corepack provides the
correct tool, no bypass needed.

## 3. Remote host bring-up

sopell3 (and any other workspace remote hosts) need a one-time:
  - `corepack enable`
  - `corepack prepare pnpm@<pinned> --activate`

Document this in AGENTS.md or a workspace-bootstrap section. If there's an
existing remote-provisioning script, add it there. Test that
`./dev.py up` (or whatever runs on the remote) handles the bootstrap
automatically via `_ensure_corepack_pnpm()` — ideally remote setup is
zero-touch beyond having node installed.

## 4. Build-worktree handling

`./dev.py prod deploy` uses `/Users/scott.opell/dev/.phoenix-ide-build/` as
a separate checkout. The drift-warning at dev.py L1332-1345 talked about
"uncommitted lockfiles" — re-evaluate whether that warning is still needed
once pnpm-lock.yaml is deterministic. Likely simpler now: just ensure the
build worktree gets the same `_ensure_corepack_pnpm()` treatment.

## 5. Cleanup pass

Update everything that references npm:
  - `scripts/cut-a-version.sh`
  - `skills/phoenix-deployment/SKILL.md`
  - `AGENTS.md` (any `npm` mentions in agent instructions — update to pnpm)
  - CI config if any (`.github/workflows/`, etc. — check)
  - Any `node_modules/.package-lock.json` checks in dev.py (success-marker
    file is different for pnpm; use `node_modules/.modules.yaml` or
    `node_modules/.pnpm/lock.yaml`)

## 6. Verification (before marking task done)

- `./dev.py up` cold start works on darwin laptop (no node_modules, no
  corepack-prepared pnpm).
- `./dev.py up` cold start works on sopell3 with only `node` pre-installed.
- `./dev.py check` passes (lint + build + tests).
- `./dev.py prod deploy` succeeds end-to-end.
- Intentionally try to break it: `pnpm install` with a manually-bumped
  dep on darwin, commit the lock, run `pnpm install --frozen-lockfile`
  on linux — should succeed (this is the whole point).
- Try invoking dev.py with corepack disabled (`corepack disable`) — the
  helper should fail loudly with actionable error, not silently fall back
  to system pnpm.

# Out of scope

- Switching off rolldown / back to Rollup (user explicitly wants to stay on
  rolldown).
- Workspace-host provisioning beyond the corepack one-liner (existing
  bootstrap scripts handle node install).
- Migrating other Python tooling (unrelated).

# Notes for the implementing agent

- pnpm 9.x is current stable as of 2026-05; verify `pnpm -v` after install
  to get the exact pin.
- The `.phoenix-ide-build/` worktree is not always present — `_ensure_corepack_pnpm()`
  should be idempotent and cheap to call from any cwd.
- Test the "wrong pnpm version" failure path manually — tweak the
  `packageManager` field by one patch version and confirm dev.py rejects it.
- Do NOT add a fallback that uses system pnpm if Corepack-prepared pnpm
  isn't available. The whole point is that there's exactly one pnpm.
