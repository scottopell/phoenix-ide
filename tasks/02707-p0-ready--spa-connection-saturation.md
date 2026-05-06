---
created: 2026-05-06
priority: p0
status: ready
artifact: ui/src/
---

Phoenix becomes unresponsive multiple times per day. Root cause is
HTTP/1.1 per-origin connection pool exhaustion at the browser. The app
is unusable until the server is bounced.

## Observed symptoms

- Browser network devtools shows requests "pending forever" — they
  never reach the server. Phoenix logs show only successful polling
  during the hung period; the queued requests are invisible to phoenix.
- Affected requests: any user-initiated action — POSTing a message,
  clicking "Approve task", "Continue in new conversation", files pane
  loading, AskUserQuestion response submit.
- Occurs within minutes of a fresh page load, sometimes immediately on
  first interaction after load.
- Bouncing prod clears it. Reproducible the same session.
- Only one browser tab open during incidents.

## System-level findings

Browser enforces a hard limit of 6 simultaneous HTTP/1.1 connections
per origin. Each long-lived connection (SSE stream, WebSocket terminal)
permanently occupies a slot. On initial SPA load, `ss -tn` shows 7-9
ESTABLISHED connections from the client IP to phoenix:8031 — at or
past the limit before the user has done anything.

```
# Captured during incident (1 browser tab open):
ESTAB  172.17.0.2:8031  10.126.65.178:63420
ESTAB  172.17.0.2:8031  10.126.65.178:63421
ESTAB  172.17.0.2:8031  10.126.65.178:63422
ESTAB  172.17.0.2:8031  10.126.65.178:63423
ESTAB  172.17.0.2:8031  10.126.65.178:63424
ESTAB  172.17.0.2:8031  10.126.65.178:63425   ← 6 opened in rapid succession
ESTAB  172.17.0.2:8031  10.126.65.178:63467
ESTAB  172.17.0.2:8031  10.126.65.178:64211
```

12 SSE stream-open requests logged for 5 different conversation IDs
within 30 minutes of a fresh session with a single tab. Implies the
SPA is opening SSE streams for conversations other than the one
currently visible.

Log confirms server health during hung periods — every request that
reaches phoenix responds in 0-5ms. The problem is entirely client-side
connection pool exhaustion.

## What has been fixed (not the problem)

- `TCP_USER_TIMEOUT=60s` on the listener socket: reaps phantom
  connections (client closed but TCP FIN lost in transit) in ≤60s
  instead of ~15 min. Eliminates one accumulation source but not the
  structural saturation on load.
- Stale WARN log spam from a dead deserialization path in the SSE
  state_change handler: fixed separately, unrelated to this issue.

## What has NOT been fixed

The SPA opens more long-lived connections than Chrome's HTTP/1.1 limit
allows, on a single tab, on normal use. Exact source not yet traced:
could be multiple simultaneous SSE subscriptions, terminal WebSocket
held open across navigation, parallel initial-load fetches that don't
release back to the pool, or a combination.

## Goal

Triage the SPA against SSE/HTTP/1.1 best practices:

- [ ] Determine exactly how many long-lived connections the SPA opens
      on initial load and what each one is for
- [ ] Identify whether SSE streams are opened for off-screen
      conversations (confirmed suspicious from log evidence above)
- [ ] Verify SSE and WebSocket connections are closed on navigation,
      not leaked
- [ ] Measure connection count under normal use: load page, open one
      conversation, send a message — how many ESTABLISHED sockets?
- [ ] Reduce to ≤3 long-lived connections per tab under normal use,
      leaving headroom for user-initiated requests

## Longer-term mitigation already tracked

02705 (HTTP/2 + TLS): multiplexes all streams over one connection,
eliminates the per-origin limit entirely. Correct structural fix but
requires TLS. This p0 is the HTTP/1.1 triage that unblocks daily use
while 02705 is built.
