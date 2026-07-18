# Development loop reference

Load this when the top-level loop needs command or source-routing detail.

## Orient without wandering

```bash
git status --short --branch
./dev.py status
```

1. Search for exact symbols, routes, error strings, UI labels, and nearby tests.
2. Find the owning feature under `specs/`; do not infer behavior from filenames alone.
3. Read bounded regions around matches. Use `keyword_search` only when you lack precise terms.
4. Use parallel sub-agents for genuinely independent surfaces, then synthesize one model. Pass a valid worktree path and an available model.
5. Check `git log`/`blame` when current shape or a suspicious compatibility path needs rationale.

Do not start servers by ritual. Start them when runtime or browser validation needs them.

## Source-of-truth routing

| Question | Authority |
|---|---|
| What must the user/system receive? | `specs/<feature>/requirements.md` |
| Exact lifecycle, ordering, pre/postconditions, invariants | `specs/<feature>/*.allium` |
| Why was a design chosen? | `specs/adrs/` shared decision chain |
| What is implemented/verified now? | `specs/<feature>/executive.md` |
| What does code do now? | Implementation + tests; disagreement with a normative spec must be resolved |
| What crosses the SSE boundary? | Rust `SseWireEvent` and parity tests; generated TS is derived |
| What persists? | SQLite DDL/migrations and typed domain rows, not serde convenience |
| How is work tracked? | `taskmd`; task filename is metadata source of truth |

Before pushing a spec change, run the checklist in `specs/AUTHORING.md`. Timeless requirements/Allium contain no rollout status, task references, resolved-question logs, or rotting line citations.

## Commands by intent

### Environment

```bash
./dev.py up          # Start Phoenix + Vite; seeds an empty worktree DB
./dev.py restart     # Rebuild/restart Rust; Vite remains running
./dev.py status
./dev.py down
./dev.py reap --dry-run
```

Every worktree gets isolated ports and a database. Use `phoenix.log` for dev logs. For production operations, invoke `phoenix-deployment` and inspect `~/.phoenix-ide/prod.log`; do not use dev assumptions as production evidence.

### Focused validation

```bash
cargo test <module-or-test-filter>
cargo test proptests
cargo test -- --nocapture
cd ui && pnpm test -- <test-file-or-filter>
./dev.py codegen
```

Use the package manager pinned in `ui/package.json`; do not invent a separate Node bootstrap.

For conversation API behavior, prefer `phoenix-client.py`. Use browser tools when the claim concerns rendered UI, focus, responsive layout, scrolling, timing, or a complete user journey. Check console errors after focused interactions.

### Repository check

```bash
./dev.py check
./dev.py check --all
./dev.py check --lanes <comma-separated-lanes>
./dev.py --pretty check
```

Default local checks are path-gated. Use `--all` when broad confidence is required or gating may miss a cross-cutting impact. CI requires explicit `PHOENIX_CHECK_ALL=1` or `PHOENIX_CHECK_BASE`; do not emulate CI with an implicit base.

### Tasks and commits

```bash
echo 'Body' | ./dev.py taskmd new --slug <slug> --priority p2
./dev.py taskmd status <id> in-progress
./dev.py tasks validate
git diff --check
git diff --stat
git diff
git commit -m 'fix: ...'
```

Capture off-path findings instead of silently dropping them. During active work, keep lightweight in-conversation TODOs; use `taskmd new` for work that will survive this session.

## Widening validation rings

1. **Regression:** the smallest test or reproducer that fails without the fix.
2. **Owner:** related module/crate/component tests and static checks.
3. **Boundary:** provider, DB, SSE/codegen, API, browser, or lifecycle contract tests.
4. **Journey:** real rendered/API/operational behavior when the user-visible claim needs it.
5. **Repository:** `./dev.py check`; use `--all` when impact is cross-cutting.
6. **Integration:** after rebase/review, rerun tests around touched conflict seams—not only the check that passed before integration.

When checks fail, classify before acting:

- **Introduced:** caused by the patch; fix it.
- **Exposed:** pre-existing defect made visible by the patch; fix if on-path, otherwise capture and explain.
- **Unrelated blocking:** not caused by the patch but prevents required proof; investigate enough to make ownership explicit.
- **Unrelated non-blocking:** record evidence and continue only when required checks still establish the claim.

Never delete regression coverage or weaken assertions to obtain green.

## Frequent boundary traps

### Rust ↔ TypeScript SSE

Change typed Rust wire structures first, run `./dev.py codegen`, update valibot schemas/consumers, and run parity tests. A hand-edited generated file is not a fix.

### Persistence

If SQL must address a value, give it a column or child row. A migration using `json_extract`/`json_set` is evidence the value wanted schema. Test old-row loading, migration, write/read round trips, and crash/restart behavior where applicable.

### Tools/providers

Read the tool/provider spec before editing. Preserve all typed data to the next capable layer. If a backend cannot represent a capability, make the gap structural where possible and log any drop at `debug` or above.

For a new tool, start at `crates/phoenix-tools/src/think.rs`, implement `Tool`, register it in `crates/phoenix-tools/src/lib.rs`, and add the appropriate spec. Do not use the stale pre-workspace `src/tools.rs` paths.

### UI

Colocate component/page CSS with its owner; leave only global shell/primitives in `ui/src/index.css`. Validate keyboard ownership, overlay/focus scopes, loading/error/empty states, narrow viewport behavior, and reconnect/replay when relevant. Do not add duplicate status UI.

### Git/worktrees

Fetches are safe; local branch ref movement is not safe when that branch is checked out in any worktree. Distinguish the current conversation worktree, repository root, and user-owned worktrees before cleanup or branch operations.

## Recovery rules

- **Missing artifact:** confirm `pwd`, worktree, branch, and repository state before widening search.
- **Ambiguous patch:** reread current text and anchor on unique surrounding structure; do not loop the same patch.
- **Repeated test/tool failure:** update the hypothesis or environment model before retrying.
- **Local/production mismatch:** query bounded production traces/logs and collector warnings; do not assume deployed code equals the checkout.
- **Spec/code conflict:** stop and identify the intended contract. Change a normative spec only when the product decision changed.
- **Review finding after “done”:** reopen the failure model, add regression evidence, and commit the hardening as a logical unit.
