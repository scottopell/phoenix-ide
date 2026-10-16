# Expose Phoenix browser tools through a standalone CLI and session daemon

## Dependency

Task 41015 (`browser-accessibility-snapshots`) landed on main in [PR #923](https://github.com/scottopell/phoenix-ide/pull/923), merge commit [`7b12499fdcfd2c51f7ec998ca53b5c6e9aac1053`](https://github.com/scottopell/phoenix-ide/commit/7b12499fdcfd2c51f7ec998ca53b5c6e9aac1053). Build on that implementation; do not duplicate snapshot/reference behavior on a parallel branch.

## Goal and user scope

Make Phoenix's browser operations usable from other coding harnesses through a `phoenix-browser` executable. Use the agent-browser pattern: short-lived CLI requests reach a lazily started local daemon for a selected session. Focus on standard coding-agent workflows: fresh browser, navigation, inspection, interactions, debugging, and performance profiling.

The user explicitly chose two independently landed units: improve Phoenix accessibility snapshots first, then build this entrypoint on top. KISS and a pit of success take priority over generalized browser infrastructure.

## Agreed session contract

- Resolve session identity in this order: explicit `--session`, environment binding, then a deterministic Git-worktree-root default; outside Git, use the current directory. Different subdirectories of one worktree reuse a session; distinct worktrees do not. Support ordinary clones and linked worktrees.
- A harness can supply a unique environment binding to isolate agents sharing a worktree. Document that the fallback intentionally shares within a worktree and does not identify a conversation.
- Navigation lazily launches Chrome. Repeated CLI invocations reuse the live browser and its console/profiling state.
- Idle cleanup shuts down the daemon/browser and discards its transient profile. Active operations are not idle. Offer explicit close and a configurable idle timeout; select and document a simple default.
- State loss is explicit: after expiry, navigation can start fresh; interaction/eval/profile operations report that the previous browser state is gone and how to recover. Do not silently create an empty replacement for a stale operation.
- Distinguish a session's stable selection key from an individual browser lifetime so refs and active captures cannot cross recreation.

## Tool and output contract

- Reuse the landed snapshot/reference implementation and existing Phoenix browser operation behavior. Extract only the browser-specific boundary needed to avoid constructing a full Phoenix ToolContext or running the Phoenix server.
- Preserve navigation, eval, screenshots, viewport, click/type/key, selector waits, console retrieval/clear, and all existing profiling actions. Keep file-only help/CPU-profile summarization usable without launching Chrome.
- `open` returns final URL, title, a compact interactive accessibility snapshot with usable references, and relevant console errors. Clearly distinguish navigation success from snapshot failure and associate errors with that navigation rather than unrelated old console entries.
- Support concise human-readable output and consistent `--json` results, error codes, session identity, artifact paths, and actionable recovery messages. Provide `eval --stdin` to avoid shell-quoting problems.
- Retain profiling methodology: readiness boundaries, resets, warmups, raw per-run samples, null/unavailable distinctions, and warnings. Do not automatically retry side-effecting operations after ambiguous transport failure.
- Add concise harness/skill usage documentation showing session binding, the normal open/snapshot/interact loop, profiling, and close.

## Acceptance evidence

- Independent CLI invocations preserve page, console, and active profiling state in one session.
- Worktree-root fallback, explicit/environment precedence, separate-session isolation, and outside-Git behavior are covered.
- Concurrent initial calls cannot create two owners for one session. Session startup, close, idle expiry, and next navigation work without leaving owned Chrome processes behind.
- Long-running operations/profiling scenarios survive the idle budget; expired-session operations report loss instead of acting on a replacement page.
- Real-browser CLI journey: open -> snapshot -> interact by ref -> inspect console/screenshot -> run scenario -> save/summarize profile -> close.
- Existing Phoenix browser tests remain green; appropriate `./dev.py check` lanes and standalone packaging checks pass, with unrelated failures classified.

## Non-goals

MCP, remote hosting, sharing a live browser with Phoenix, profile cloning/import, credential extraction, auth-state restore, cloud providers, and live-view UI. Those can earn separate scope later. The daemon serves this CLI; do not build several transports speculatively.

## Starting points and adjacent existing work

- `crates/phoenix-browser/{Cargo.toml,src/lib.rs,src/session.rs}`
- `crates/phoenix-tools/src/browser/{tools.rs,profile.rs}` and `browser_tools()` registration.
- `specs/browser-tool/` and the compatibility policy before changing any guarantees.
- Existing task 45006 tracks idle-reap revalidation; inspect its current implementation/status before reusing or changing idle cleanup. Avoid duplicating it.
