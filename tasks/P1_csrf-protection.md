---
created: 2025-02-07
priority: p1
status: ready
---

# Add CSRF Protection Headers

## Summary

Add CSRF protection to state-changing API endpoints using the `X-Shelley-Request` header pattern from rustey-shelley.

## Context

Cross-Site Request Forgery (CSRF) attacks trick authenticated users into making unwanted requests. Since phoenix-ide runs on a user's machine with access to the filesystem (via bash tool, patch tool), CSRF is a real concern.

A malicious website could potentially:
- Create conversations and run commands
- Execute arbitrary bash commands if the user has an active session
- Modify or delete files via the patch tool

## The Attack Vector

1. User has phoenix-ide running at `localhost:8000`
2. User visits `malicious-site.com` in another tab
3. Malicious site sends `POST /api/conversations/new` with `{"cwd": "/", "message": "rm -rf /tmp/important"}`
4. Browser includes cookies/auth, request succeeds
5. Agent executes the command

## The Fix

Require a custom header on all state-changing requests:

```typescript
// Frontend: api.ts
const postHeaders = {
  "Content-Type": "application/json",
  "X-Phoenix-Request": "1",
};
```

```rust
// Backend: middleware
fn require_csrf_header(req: &Request) -> bool {
    req.headers().get("X-Phoenix-Request").is_some()
}
```

**Why this works:** Browsers enforce CORS for custom headers. A cross-origin request with `X-Phoenix-Request` triggers a preflight OPTIONS request, which will fail because we don't allow the foreign origin.

## Reference Implementation

From rustey-shelley `ui/src/services/api.ts`:
```typescript
private postHeaders = {
  "Content-Type": "application/json",
  "X-Shelley-Request": "1",
};
```

All POST/PUT/DELETE requests include this header.

## Acceptance Criteria

- [ ] Add middleware to reject POST/PUT/DELETE without `X-Phoenix-Request` header
- [ ] Return 403 Forbidden with clear error message when header missing
- [ ] Update all frontend fetch calls to include the header
- [ ] GET requests remain unprotected (they should be read-only anyway)
- [ ] SSE streams remain unprotected (EventSource can't set custom headers)
- [ ] Document the security model in README or SECURITY.md

## Endpoints to Protect

- `POST /api/conversations/new`
- `POST /api/conversations/:id/chat`
- `POST /api/conversations/:id/cancel`
- `POST /api/conversations/:id/archive`
- `POST /api/conversations/:id/unarchive`
- `POST /api/conversations/:id/delete`
- `POST /api/conversations/:id/rename`
- `POST /api/mkdir`

## Notes

- This is defense-in-depth; localhost services have some inherent exposure
- Consider also adding SameSite cookie attributes if we add auth later
- The header name `X-Phoenix-Request` is arbitrary; consistency matters more than the name
