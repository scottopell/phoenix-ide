---
id: 018
title: Fix ServerError (5xx) not retrying per REQ-BED-006
status: done
priority: p1
created: 2025-02-15
---

# ServerError Retry Fix

## Problem

LLM API 5xx errors (server errors) were NOT being retried, violating REQ-BED-006.
When Anthropic returned a 500 Internal Server Error, the conversation immediately
transitioned to Error state instead of retrying up to 3 times.

## Root Cause

1. `db::ErrorKind` enum was missing `ServerError` variant
2. `llm_error_to_db_error()` used catch-all `_ => Unknown` which silently swallowed `ServerError`
3. `ErrorKind::is_retryable()` only included `Network | RateLimit`, missing `ServerError`

## Fix

- Added `ServerError` variant to `db::ErrorKind`
- Added `TimedOut` to `is_retryable()` (was also missing)
- Changed `llm_error_to_db_error()` to use explicit match arms (no catch-all)
- Added comprehensive tests for error mapping and retryability
- Added `ServerError` to proptest strategies

## Verification

All 5xx errors now:
1. Map to `ErrorKind::ServerError`
2. Return `true` from `is_retryable()`
3. Trigger retry with exponential backoff (1s, 2s, 4s)
4. Only fail after 3 attempts

## Commits

- a6bedf0 - fix(critical): ServerError (5xx) now properly retries per REQ-BED-006
