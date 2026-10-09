# ADR-087: Tmux effects go through a backend seam, and tests use an in-memory fake

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** tmux registry and `tmux_run` (`specs/tmux-integration/`), Close retirement of tmux servers (ADR-039, ADR-040)

## Context

The tmux registry decides when a WorkScope's server is reused, replaced, retired, or reclaimed after a restart. Those decisions rest on exact identity: a durable server token, an exact server process identity, and the socket file's incarnation. The registry made every external effect inline: tmux CLI calls, socket probes, socket-file metadata, and process liveness.

So the only way to test those identity rules was to start real tmux servers. That needed a 2,164-line containment harness (test-owned roots, watchdogs, forced-parent-death recovery) to keep test servers from leaking. Those tests were the main source of flakes in the check gate. They failed on harness cleanup deadlines and spawn timing, not on registry logic. Hosts where every process launch is slow (endpoint-security agents) made this worse.

## Options considered

1. **Keep real-tmux tests and harden the harness further.** Each round of hardening added more process management, and the tests still depended on timing that the host controls.
2. **Delete the real-tmux tests.** Removes the flakes, but leaves the identity rules (never kill a reopened replacement, reclaim by durable identity after restart, fail closed on ambiguity) without coverage.
3. **Put the external effects behind a narrow trait, move the production code behind it without behavior change, and test the registry against a deterministic in-memory fake.** The rules keep their coverage, and the tests stop depending on processes or time.

## Decision

Choose option 3. `TmuxBackend` covers only the effects the registry and `tmux_run` need. `SystemTmuxBackend` holds the moved production code. `TmuxRegistry` holds an `Arc<dyn TmuxBackend>`; its production constructors still use the system backend. `FakeTmuxBackend` (behind `cfg(test)` or the `test-support` feature) models servers keyed by socket path, with tokens, process identities, windows, and scriptable events. Every operation answers at once.

Tests for registry, `tmux_run`, runtime, and wake behavior use the fake and assert the identity rules directly. No test starts a real tmux server. Code that only parses or classifies tmux output is tested as a pure function.

## Consequences

- **Positive:** The identity rules are tested deterministically, including cases a real server cannot easily be made to produce (an ambiguous probe, a zombie server process, a replacement at the same socket).
- **Positive:** The containment harness and its test-only hooks are gone.
- **Negative:** Nothing in the test suite runs the real tmux binary. A change in tmux's CLI output or behavior is caught only by the pure parser tests, by `SystemTmuxBackend` being a direct move of existing code, and by use in development.
- **Negative:** The fake can drift from real tmux. A new backend method needs matching fake behavior, and a fake that is too generous can hide a bug.
- **Neutral:** Tests that need a real OS process to prove a process property (a timed-out child is killed and reaped) still start one, with no timing race in the assertion.

## References

- ADR-039: Durable runtime resource identity fails closed
- ADR-040: Close uses scope gates and tmux-only durable identity
- `specs/tmux-integration/requirements.md`, `specs/tmux-integration/tmux-integration.allium`
- Key symbols: `tmux::backend::TmuxBackend`, `tmux::backend::SystemTmuxBackend`, `tmux::fake_backend::FakeTmuxBackend`, `TmuxRegistry`
