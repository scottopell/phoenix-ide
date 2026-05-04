---
name: caveman-review
description: One-line PR review comments. No throat-clearing. Severity emoji + file:line + fix.
---

# Caveman Review

Review the diff and emit findings as **one-line comments**. No
throat-clearing, no "I've taken a look at your changes", no executive
summary unless explicitly asked.

## Output format

One finding per line:

```
<file>:<line> <emoji> <category>: <issue>. <fix>.
```

- `<file>:<line>`: as it would appear in a code-navigator. Use the
  highest-impact line if a finding spans a range.
- `<emoji>`: severity. `🔴` bug / data loss / security; `🟡` risk /
  smell / future footgun; `🔵` style / nit; `🟢` looks good (use
  sparingly, only if the user asked for an LGTM-style summary).
- `<category>`: one short noun: `bug`, `race`, `null`, `leak`, `perf`,
  `style`, `naming`, `test`, `docs`, `security`, `api`.
- `<issue>`: 3-8 words on what's wrong.
- `<fix>`: 3-8 words on what to do. Omit if obvious from the issue.

## Rules

- **One line per finding.** If you can't compress to one line, the
  finding is too vague — split it or strengthen it.
- **Cite specific lines.** No "throughout this file" or "in the new
  code". Pick the canonical location.
- **No restating the diff.** Reviewers know what the code does; tell
  them what's wrong.
- **No praise.** "good naming", "nice refactor" — skip them. Silence is
  approval.
- **Group by severity, then by file.** All 🔴 first, then 🟡, then 🔵.
- **End with a one-line verdict** when the user asked for a final call:
  `Verdict: ship | block | revise`.

## Examples

```
src/auth/middleware.rs:42 🔴 bug: token expiry uses `<`, allows boundary tokens. Use `<=`.
src/api/handlers.rs:118 🟡 perf: N+1 query inside loop. Batch with `IN (?)`.
src/api/handlers.rs:201 🟡 race: read-then-write on shared map without lock. Wrap in `Mutex`.
ui/src/api.ts:87 🔵 naming: `tmp` is opaque. Call it `pendingResponse`.
Verdict: revise
```

## What to skip

- Whitespace, formatting (the linter's job).
- Subjective style preferences not flagged by the project's conventions.
- Hypotheticals not grounded in the diff.
- Repeated findings — if the same bug appears at three lines, cite the
  first line and add `(also L:55, L:67)`.

If the user passed `$ARGUMENTS` (e.g. "focus on security", "ignore
tests"), narrow the review to that scope.
