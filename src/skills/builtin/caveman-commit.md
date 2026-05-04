---
name: caveman-commit
description: Write a terse commit message. Conventional Commits, ≤50 char subject, why over what.
---

# Caveman Commit

Write a single commit message for the staged changes following these rules.
Output **only** the commit message — no preamble, no explanation, no
"here's your commit message" framing.

## Format

```
<type>(<scope>): <subject>

<body>
```

- `<type>`: one of `feat`, `fix`, `refactor`, `perf`, `docs`, `test`,
  `build`, `chore`, `tasks`. Match the prefix style of the repo's recent
  commits if it differs.
- `<scope>`: optional, lowercase, single token. Omit for cross-cutting
  changes.
- `<subject>`: ≤50 chars, imperative mood ("add", "fix", not "added",
  "fixes"), lowercase, no trailing period.
- `<body>`: optional. One short paragraph explaining **why**, not what
  (the diff already shows what). Wrap at 72 chars. Skip entirely for
  trivial changes.

## Rules

- **Why over what.** "fix race in worker shutdown" beats "change Drop impl
  on Worker". The subject conveys the user-visible effect or intent; the
  body conveys the reasoning if non-obvious.
- **No filler.** Drop "this commit", "this change", "this PR", "I have",
  "we now". Imperative is the entire voice.
- **One concern per commit.** If the diff spans two unrelated concerns,
  emit two commit messages separated by a `---` line and call it out
  explicitly.
- **No emoji**, no "🎉", no co-author tags unless the diff already shows
  multi-author work.
- **Match the log style.** If the recent log uses different conventions
  (e.g. no scope, different types), follow the log over these defaults.

## Workflow

1. Read recent commit log: `git log --oneline -20`. Note prefix style and
   subject conventions.
2. Read the staged diff: `git diff --cached`. Identify the user-facing
   change.
3. If staging is empty, read the unstaged diff: `git diff`. Note that
   nothing is staged yet.
4. Emit the message in a fenced block. Nothing before, nothing after.

## Examples

```
fix(auth): reject expired tokens on refresh

The refresh path checked exp with `<` instead of `<=`, so tokens
that expired at exactly the boundary were accepted for one extra
millisecond. Caught by the integration test added in 0091.
```

```
refactor: collapse SkillSource into enum
```

```
build: pin tower-http to 0.5.2
```

If the user passed `$ARGUMENTS` (e.g. extra context, a ticket number, or a
"include co-author X" directive), incorporate it without breaking the
above rules.
