---
created: 2026-04-12
priority: p2
status: ready
artifact: pending
---

# clippy-pedantic-tools-patch-proptests

## Problem

`./dev.py check` runs `cargo clippy` with `-D clippy::pedantic`, which
flags several warnings in `src/tools/patch/proptests.rs` as errors.
These pre-date the terminal HUD work in this session (task 24664-24666)
but have been quietly blocking `./dev.py check` for anyone who tries
to run the full check suite in isolation.

Observed during task 24666's QA subagent run:

```
cast_possible_truncation
string_slice
```

(exact lines to be pulled when working the task — the warnings are
stable across runs)

## Scope

Fix the clippy pedantic warnings in `src/tools/patch/proptests.rs`.
Options per warning site:

- `cast_possible_truncation`: use `try_from`, `#[allow]` with a
  justifying comment if the cast is provably safe, or change the
  types to avoid the cast entirely
- `string_slice`: replace raw byte slicing with char-aware APIs
  (`str::get`, `str::chars().take()`), or `#[allow]` with a
  comment explaining why the byte-slice is correct for the context

Prefer fixes over allows where the fix is mechanical. Default to
allow with a one-line justification only when the alternative is
convoluted.

## Out of scope

- Broad clippy pedantic cleanup across the repo. This task is
  scoped to `tools/patch/proptests.rs` specifically because that's
  the remaining blocker for `./dev.py check` after task 24666
  cleaned up `src/llm/mock.rs`.
- Disabling `clippy::pedantic` globally. It's on for a reason.

## Related

- Task 24666 fixed the parallel set of pedantic warnings in
  `src/llm/mock.rs` as part of landing mock LLM provider work
