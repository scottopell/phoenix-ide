---
created: 2026-04-08
priority: p2
status: in-progress
artifact: src/message_expander.rs
---

# @ references that don't resolve to files should not block message send

## Problem

When a message contains `@` followed by text that isn't a valid file path,
the expansion fails with `FileNotFound` and the message is blocked from
sending (HTTP 422). This happens for email addresses, `@mention` style
text, `@param` annotations in code snippets, or any `@` usage that isn't
an intentional file reference.

The current behavior (REQ-IR-007) assumes every `@` token is an intentional
file reference. In practice, `@` is common in normal text and the false
positive rate is high enough to be disruptive.

## Proposed fix

Change the expansion behavior for `@` references that don't resolve:
- If the path after `@` looks like a plausible file path (contains `/`
  or `.` with a known extension) AND doesn't resolve: block send with
  error (current behavior, intentional reference that's broken)
- If the path after `@` does NOT look like a file path (no `/`, no file
  extension, looks like a word/name): pass through as literal text without
  expansion, do not block send

This preserves the safety of REQ-IR-007 for real file references while
eliminating false positives for casual `@` usage.

## Spec impact

REQ-IR-007 in specs/inline-references/requirements.md needs updating to
distinguish between "looks like an intentional file reference" and "happens
to contain @". The tokenizer in message_expander.rs already has boundary
checks (@ must be at start or after whitespace) but doesn't check whether
the token looks like a path.

## Done when

- [ ] `@username` in a message sends without error
- [ ] `user@email.com` in a message sends without error
- [ ] `@src/main.rs` that doesn't exist still shows an error
- [ ] `@AGENTS.md` that exists still expands correctly
