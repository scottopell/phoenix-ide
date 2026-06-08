Inline-reference errors are not reset when the new-conversation branch/mode changes.

The new-conversation composers inline-reference state is keyed only by `conv.cwd`, but the
actual resolution root now also depends on `conv.submission.mode` and
`conv.submission.baseBranch`. If a send fails because `@foo` is missing on one selected
branch, then the user switches to another branch in the same repo where `@foo` exists,
`ir.expansionError` stays set and BOTH the click and Enter send paths remain disabled
until the user edits the composer or manually dismisses the stale error.

## Fix
Include mode + base branch in the inline-reference scope key (so the state resets when the
resolution root changes), or clear `expansionError` when mode/baseBranch change. See
`ui/src/pages/NewConversationPage.tsx` (~line 80).

## Context
Surfaced by codex review on PR #232. Codex rated it P3; filed as P1 per maintainer
request (it silently blocks sending).
