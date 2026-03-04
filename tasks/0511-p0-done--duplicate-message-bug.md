---
created: 2026-02-05
priority: p0
status: done
---

# P0: Critical - Messages Sent Twice When Retrying Failed Send

## Status: FIXED ✅

Fixed in commits:
- c6b58a6: Add idempotent message sends with local_id
- de06274: Fix database migration order

## Root Cause

The client generated a `localId` for local tracking but **never sent it to the server**. The server had no way to deduplicate requests - if the same message content was sent twice, it created two messages.

The client-side deduplication via `sendingMessagesRef` had race conditions between the retry state change triggering the effect and the actual send adding to the ref.

## Fix: Correct By Construction

Made the server **idempotent** - duplicate sends are now impossible at the data layer:

1. **API Change**: Added `local_id` (required) and `user_agent` (optional) to `ChatRequest`
2. **Database Change**: Added `local_id` column with unique constraint on `(conversation_id, local_id)`
3. **Handler Logic**: Check for existing message with same `local_id` before processing, return success if duplicate
4. **Client Change**: Send `localId` and `navigator.userAgent` with message requests

## Verification

Tested by sending same message twice with identical `local_id`:
```bash
# First send - creates message
curl -X POST .../chat -d '{"text": "Test", "local_id": "test-123", ...}'
# {"queued":true}

# Second send - idempotent, no duplicate
curl -X POST .../chat -d '{"text": "Test", "local_id": "test-123", ...}'
# {"queued":true}

# Only ONE message in database
SELECT COUNT(*) FROM messages WHERE local_id = 'test-123';
# 1
```

Server log confirms duplicate detection:
```
"Duplicate message detected, returning success (idempotent)"
```

## Why This Is Correct By Construction

- **Impossible to create duplicates**: Database constraint enforces uniqueness
- **No race conditions matter**: Even if client sends same message 100x, only one row created
- **Retry is always safe**: Network failures, timeouts, UI bugs - all harmless
- **Simple mental model**: "localId is the message identity; server is idempotent"

## Bonus: User Agent Tracking

As a small UX enhancement, the client now sends `navigator.userAgent` which is stored in `display_data`. This enables showing device icons (iPhone, desktop, etc.) next to messages in the UI.
