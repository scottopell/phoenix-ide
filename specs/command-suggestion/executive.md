# Command Suggestion — Executive Summary

## Problem

A user in a Phoenix terminal wants to ask, in plain language, for the commands
to do something ("scaffold these worktrees", "what's my build command") and run
them — without leaving the terminal or hand-writing the commands.

## Solution

A `phx` command, guaranteed on every terminal's `PATH`, sends the request to a
stateless `POST /api/suggest` endpoint (a single tool-less LLM completion) and
renders the suggested commands as OSC 8 click-to-run links. Activating a link
drops the command onto the shell prompt for the user to review and run — never
auto-executed.

`phx` is a symlink to the running server binary (no second artifact, no
interpreter dependency). The endpoint is authorized by a scoped capability
token injected into the terminal environment, persisted across restarts and
bound to the password fingerprint.

## Scope

**In:**
- `POST /api/suggest`: stateless, tool-less, one-shot suggestion.
- Capability-token auth (`X-Phoenix-Suggest-Token`), exempt from password.
- Token lifecycle: mint → persist (`app_settings`) → reuse → rotate on
  password change.
- The `phx` PATH shim (symlink + argv dispatch) and PTY env injection (shell
  and tmux paths).
- OSC 8 `phxrun:` click-to-run links.

**Out / deferred:**
- Retrieval-augmented suggestion scoped to a terminal's conversation lineage.
- A full agent turn for suggestions that need tools.
- Persisting suggestions or surfacing them outside the terminal.

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Backing call | Stateless one-shot `complete()`, no tools | Suggester not agent; cannot execute; no transcript pollution |
| Default model | Cheap tier (`get_cheap_model`) | Latency-sensitive, low-stakes; mirrors title generator |
| Auth | Scoped capability token, password-exempt | `phx` has the token, not the password; works with or without auth |
| Token durability | Persisted, fingerprint-bound | Survives restarts (terminals stay authorized); rotates on password change |
| `phx` delivery | Symlink to the server binary | One guaranteed-present artifact; no interpreter dependency |
| Run affordance | OSC 8 `phxrun:` link → prompt, no newline | Suggestion with a human review beat; runs in the user's own shell |

## Status Summary

| Requirement | Status | Code anchor |
|---|---|---|
| REQ-CSUG-001: One-shot suggestion endpoint | Done | `api::handlers::suggest_handler`, `crate::suggest::suggest_commands` |
| REQ-CSUG-002: Model selection | Done | `suggest_handler` (`get_cheap_model` / `get`) |
| REQ-CSUG-003: Capability-token authorization | Done | `suggest_handler` token check; `api::auth::is_exempt_path` |
| REQ-CSUG-004: Token lifecycle | Done | `resolve_suggest_token`, `mint_suggest_token` (main.rs) |
| REQ-CSUG-005: Guaranteed `phx` on PATH | Done | `phx_cli::is_cli_invocation`, `install_phx_symlink` |
| REQ-CSUG-006: PTY environment injection | Done | `phoenix_terminal::spawn` `PtyEnvInjection`; tmux `apply_pty_env_injection` |
| REQ-CSUG-007: Click-to-run suggestion links | Done | `phx_cli::run`; `TerminalPanel` `linkHandler` |
| REQ-CSUG-008: Context graded by availability | Done (Tier 0) | Tiers 1–2 deferred |
