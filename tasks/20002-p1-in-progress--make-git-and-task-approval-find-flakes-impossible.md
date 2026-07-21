# Make Git and task-approval find flakes structurally impossible

## Observed journey

- A full `./dev.py check`/nextest run reached the end of the Rust suite with 2641 passing tests, then two tiny Git-dependent tests hung for 8–9 minutes until the harness cancelled them:
  - `phoenix_ide api::git_handlers::tests::checkout_status_detached_reports_pointing_refs`
  - `phoenix_ide runtime::executor::commission_review_approval_tests::commission_review_approval_computes_diff_stats`
- The cancelled commission-review test emitted `Nvim: Caught deadly signal 'SIGTERM'`, indicating a child process entered an editor before the harness killed it.
- A separate Vitest failure timed out in `TaskApprovalReader.test.tsx` while opening task-approval find and searching inline Markdown text: `Use \`alpha\` and [alpha](https://example.test) safely.`
- The user suspects the Git failures are environment-related and wants either removal/refactor that makes flakes impossible by construction, or evidence that the failures are environmental rather than load-timing flakes.

## Verified findings

- Both Rust failures are tests that construct temporary Git repos and invoke Git CLI operations (`init`, `config`, `commit`, `tag`, `checkout`, `rev-parse`, `diff`) rather than runtime timers or async coordination.
- `crates/phoenix-core/src/git.rs::command()` currently only guarantees `commit.gpgsign=false` plus preservation/cleanup of indexed `GIT_CONFIG_*` state. It does not structurally neutralize editor/pager/prompt hooks such as `GIT_EDITOR`, `VISUAL`, `EDITOR`, `GIT_PAGER`, `PAGER`, or terminal prompt settings.
- `crates/phoenix-ide/src/runtime/executor.rs::git_capture()` adds `--no-optional-locks`, `GIT_OPTIONAL_LOCKS=0`, and `GIT_NO_LAZY_FETCH=1`, but the test helpers `git_ok`/`git_output` and `api::git_handlers` test setup rely on the shared Phoenix git command path or `git_ops::run_git` without a process-level noninteractive contract.
- `TaskApprovalReader.tsx` computes canonical Markdown display blocks with `buildMarkdownDisplayBlocks(plan)`, then separately asks `ReactMarkdown` to render the plan and decorates rendered React children via `decorateFindChildren`. That decoration recursively clones arbitrary valid React elements and only updates a shared text cursor for string children.
- The failing UI test exercises exactly the boundary between canonical Markdown text projection and renderer-owned inline Markdown elements (`code` and `a`). This is likely the same failure class as the prior fix in `fdf0c28fc7763a1adc013d46fd6134e4e5b54bf5`, but in the task-approval Markdown adapter rather than the shared find state machine.

## Inferences and unknowns

- Inference: the Git hangs are environmental editor/pager/prompt leakage, not load-sensitive timing. Falsify by reproducing under a sterile Git environment; if it still hangs, inspect specific Git subcommand and locks.
- Inference: the UI timeout is a render/update loop or pathological React tree traversal caused by decorating renderer-owned Markdown children after projection. Falsify by isolating `decorateFindChildren` with the inline code+link shape or by proving timers/fake timers are the blocker.
- Unknown: whether the prior `fdf0c28fc7763a1adc013d46fd6134e4e5b54bf5` fix touched shared `viewer-find` state only or also attempted to solve task-approval Markdown decoration. The current code still leaves the task-approval adapter capable of rewalking/cloning arbitrary rendered children.

## Interaction map

- Git tests: test helper → `phoenix_core::git::command()` / `git_ops::run_git()` → host Git process → inherited host environment/config → potential editor/pager/prompt process.
- Checkout status path: temp repo setup → live checkout observation → detached HEAD refs (`refs/heads/main`, `refs/tags/v1`) → `CheckoutStatus::Detached` response.
- Commission-review path: temp repo setup → `resolve_commission_review_approval` → clean worktree, base/head verification, merge-base, numstat → `CommissionReviewApprovalScope` diff stats.
- Task approval find path: `TaskApprovalReader` plan text → Markdown display block projection → find session matches → `ReactMarkdown` rendered children → ad hoc decoration/scroll reveal → DOM assertions.

## Proposed scope

### Git noninteractive-by-construction fix

- Add a single shared Phoenix Git subprocess constructor contract that makes interactive Git behavior unrepresentable for Phoenix-owned Git commands:
  - disable editors and pagers (`GIT_EDITOR`, `VISUAL`, `EDITOR`, `GIT_PAGER`, `PAGER`, `core.editor`, `core.pager` as appropriate);
  - disable terminal prompts/askpass where safe for noninteractive operations (`GIT_TERMINAL_PROMPT=0`, SSH askpass cleanup if relevant);
  - preserve required config layering while making dangerous inherited editor/pager prompt state structurally unavailable;
  - document any capability gap via tests rather than comments-only discipline.
- Route the two failing tests’ helper Git calls through that constructor or a dedicated noninteractive test Git helper, avoiding bespoke environment handling in each test.
- Add regression tests in `phoenix-core::git` proving hostile editor/pager environment and local `core.editor` configuration cannot make Phoenix-owned commit/status/diff-style commands spawn an editor or hang.
- Prefer removing Git subprocess dependence from these two tests where practical:
  - for checkout status, separate pure mapping from `LocalGitHeadObservation` / pointing ref data into status and test that pure path;
  - for commission-review diff stats, isolate diff-stat parsing and approval-scope assembly so only one integration test needs real Git.

### TaskApprovalReader find structural fix

- Refactor task-approval Markdown find decoration so matching and rendering share one typed source of truth, instead of walking arbitrary `ReactMarkdown` children with a mutable cursor.
- Make inline Markdown preservation and exact match marking representable by a local, bounded decorator for known inline text nodes/elements, or by rendering from the parsed Markdown AST blocks used for search.
- Ensure decoration is idempotent: already-decorated `mark` nodes cannot be reprocessed as source text and active-match changes cannot cause recursive wrapping.
- Keep Mermaid behavior: no-match mermaid renders through `MermaidDiagram`; active mermaid match renders exact source with marks.

## Validation

- Run the targeted Rust tests repeatedly under hostile environment/config values, including at least:
  - `GIT_EDITOR=nvim`, `VISUAL=nvim`, `EDITOR=nvim`, `PAGER=nvim`, `GIT_PAGER=nvim`;
  - repo-local `core.editor=nvim` and `core.pager=nvim`.
- Run targeted nextest filters for:
  - `checkout_status_detached_reports_pointing_refs`
  - `commission_review_approval_computes_diff_stats`
  - existing `phoenix_core::git` command tests.
- Run targeted Vitest for `ui/src/components/TaskApprovalReader.test.tsx`, including the inline Markdown test and surrounding task-approval find tests.
- Run `./dev.py check` or the relevant gated lanes after targeted fixes.

## Risks and non-goals

- Non-goal: changing user-driven terminal/bash Git behavior. The noninteractive contract should apply to Phoenix-owned subprocesses, not the user’s interactive terminal.
- Non-goal: removing all Git integration coverage. Keep a small number of deterministic integration tests for real Git semantics.
- Risk: overly aggressive environment cleanup could break legitimate noninteractive auth or config behavior. The fix should distinguish editor/pager/prompt affordances from required Git config and remote/auth mechanisms.
- Risk: task-approval Markdown rendering has deliberate differences from the file `MetaViewer` stack; reuse only where the type contract fits, not by merging unrelated lifecycle concerns.
