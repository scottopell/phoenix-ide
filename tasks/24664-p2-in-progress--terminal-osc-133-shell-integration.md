---
created: 2026-04-12
priority: p2
status: in-progress
artifact: pending
---

# terminal-osc-133-shell-integration

## Problem

The terminal HUD currently infers state by scraping the xterm buffer and
running an activity-heuristic timer. This is lossy: powerline two-line
prompts collapse to `╰─`, the idle/running dot decays on a 500ms timer
rather than reflecting real shell state, and there is no way to surface
command text, exit codes, or command duration.

OSC 133 (FinalTerm shell integration) is the de-facto standard for
structured prompt/command semantics. When emitted by the shell, it lets
the terminal know exactly when a prompt renders, when a command begins,
and when it finishes with an exit code. OSC 7 pairs with it to report
the current working directory.

Most shells do not emit these markers by default. We will detect the
presence of OSC 133 per session and either use the rich data or fall
back to the existing sampler heuristic, prompting the user with a
tailored snippet when integration is absent.

## Scope (v1, YAGNI)

**In scope:**

- REQ-TERM-015: OSC 133 detection (5s window, monotonic)
- REQ-TERM-016: Command lifecycle from A/C/D markers (B accepted but unused)
- REQ-TERM-017: Absent-case hint on the live dot, shell-tailored snippet
- REQ-TERM-018: OSC 7 cwd reporting with fallback to conversation cwd
- Rich HUD: dot + cwd + command + `✓/✗` result with duration
- Plumb `$SHELL` through the conversation API
- Shell snippet bundles for zsh, bash, fish (single paste: OSC 133 + OSC 7)

**Out of scope (explicit YAGNI):**

- Nested subshell OSC 133 interleaving
- Multi-line command text preservation (first-line truncate)
- Shell replacement via `exec`
- Historical command log beyond `last_completed_command`
- Auto-injection via `ZDOTDIR` / `--rcfile` (deferred spec entry)

## Deliverables

1. **Spec updates**
   - `specs/terminal/requirements.md`: REQ-TERM-015 through REQ-TERM-018
   - `specs/terminal/terminal.allium`: external entity, value type, enum,
     Terminal entity extensions, config, rules, invariants, surface,
     deferred section update

2. **Backend**
   - `$SHELL` value surfaced via the conversation API response (pulled
     from env or `TerminalHandle` if available)
   - No Rust-side OSC parsing — xterm.js handles the parse in the browser

3. **Frontend**
   - `TerminalPanel.tsx`: `registerOscHandler(133, ...)` and
     `registerOscHandler(7, ...)` wiring
   - Detection state machine: `unknown | detected | absent` with 5s
     timeout
   - Rich HUD rendering for detected state (idle / running / success /
     failed variants)
   - Fallback to existing sampler when absent
   - Tooltip on the live dot when absent + click-to-open modal with
     shell-tailored snippet
   - Shell snippet consts for zsh, bash, fish

4. **Task file**: this document, flipped to `done` on completion

## Commit Plan

1. Spec + task file (this commit) — `in-progress`
2. Implementation (backend + frontend, possibly split)
3. Task status → `done`

## Open Questions

None at task creation. All design decisions captured in the spec
additions via the parent conversation (2026-04-12).

## Related

- Parent spec: `specs/terminal/terminal.allium`
- Previous deferred entry: `ShellIntegration.osc133_markers` (replaced
  by REQ-TERM-015 through -018 in this task)
- Remaining deferred: `ShellIntegrationAutoInjection`
