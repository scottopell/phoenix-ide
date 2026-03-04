---
created: 2026-02-11
priority: p0
status: done
---

# Task 544: LLM Response Must Have Content (Type Safety)

## Summary

`LlmResponse` could be constructed with empty content, violating the invariant that every
LLM response must contain at least text content or tool calls. Fixed with runtime
validation in all provider `normalize_response()` methods.

## Context

When AI Gateway returned a malformed response, `normalize_response()` built an empty
content vec and returned `Ok(LlmResponse { content: vec![], ... })`. The state machine
treated this as a valid completion and transitioned to Idle, causing silent failures.

The spec proposed three long-term solutions (NonEmptyVec, validated constructor, split
response types) but the runtime validation hotfix is sufficient — all three providers
now reject empty responses, and 18 property-based tests verify the invariant holds.

## Acceptance Criteria

- [x] All LLM providers validate responses are non-empty
- [x] Clear error message when API returns no content
- [x] Property-based tests verify the invariant
- [x] No silent failures to Idle state

## Notes

- This is the root cause of task 543
- Runtime validation chosen over compile-time NonEmptyVec (sufficient for our needs)
- Fixed in commits a310394 and 9d568af
