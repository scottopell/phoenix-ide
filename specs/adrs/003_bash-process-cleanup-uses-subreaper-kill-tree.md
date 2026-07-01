# ADR-003: Bash process cleanup uses subreaper plus shutdown kill-tree

- **Status:** Accepted
- **Date:** 2026-06-29
- **Affects:** REQ-BASH-003, REQ-BASH-006, REQ-BASH-007

## Context

The bash tool runs non-interactive shell commands as children of the Phoenix
process. Those commands can start descendants of their own, including daemons
that double-fork or create new sessions. A cleanup strategy that only watches the
original child process or relies on terminal hangup behavior can leave escaped
descendants running after Phoenix exits.

The tool also has two different cleanup moments with different semantics:
user-requested `kill`, where the agent is still in control and may need graceful
shutdown, and Phoenix shutdown, where the server is exiting and cannot continue
supervising process cleanup.

## Options considered

1. **Rely on parent death or SIGHUP cascade.** This keeps Phoenix simple, but it
   does not reliably clean non-TTY descendants of a dying server process. Escaped
   descendants can reparent outside Phoenix and survive the server.
2. **Persist process state and reconcile on next startup.** This would let
   Phoenix report structured restart outcomes, but it would not itself kill
   escaped descendants before exit and would introduce a second persistence path
   alongside `tmux`.
3. **Use Phoenix as a child subreaper and kill live process groups at shutdown.**
   Descendants that escape their original parent reparent to Phoenix, and shutdown
   performs a forceful kill-tree pass over live bash handles before the server
   exits.
4. **Auto-escalate user `kill` from TERM to KILL.** This makes individual kill
   calls more forceful, but it changes the meaning of an agent's requested signal
   and can corrupt services that need a longer graceful shutdown path.

## Decision

Use Phoenix as the child subreaper for bash descendants and run a forceful
shutdown kill-tree pass over live bash process groups before Phoenix exits.
Shutdown cleanup uses `SIGKILL` because Phoenix is already exiting and cannot
wait indefinitely for graceful termination.

Keep normal user-requested `kill` operations explicit. A `kill` call sends the
requested signal exactly once and does not auto-escalate from TERM to KILL. If a
process remains alive after the kill response window, the tool returns
`kill_pending_kernel`; an agent that wants escalation must make a later explicit
`signal=KILL` request. The shutdown kill-tree is the forceful server-exit cleanup
path, not a hidden escalation path for ordinary kill calls.

## Consequences

- **Positive:** bash descendants that escape the immediate process group are
  still collected under Phoenix when possible instead of silently surviving under
  init.
- **Positive:** server shutdown has a bounded, forceful cleanup path for live
  bash handles.
- **Positive:** user-requested graceful termination keeps its exact meaning;
  agents decide when to escalate.
- **Negative:** the cleanup design depends on Linux subreaper semantics and a
  platform-specific shutdown path.
- **Negative:** shutdown cleanup is intentionally forceful and may interrupt
  child processes that would have preferred graceful termination, because Phoenix
  is no longer available to supervise them.
- **Neutral:** cross-restart process persistence remains outside bash; agents use
  `tmux` for commands that must survive Phoenix restart.

## References

- Related ADRs: ADR-001, ADR-002
- Feature spec: `specs/bash/requirements.md`
- Behavioural spec: `specs/bash/bash.allium`
- Executive summary: `specs/bash/executive.md`
