---
created: 2026-05-06
priority: p3
status: ready
artifact: src/api/sse.rs
---

Surface stale SSE clients so silent connection leaks get caught early
instead of being discovered when a user hits saturation symptoms days
or weeks later.

## Motivating example

2026-05-06: while triaging the user's second "phoenix unresponsive"
incident, found two leaked SSE streams from headless Chrome processes
holding open connections to phoenix on `localhost`:

- One session 4 days, 20 hours old (parent CLI still alive, idle)
- One session 20 days old (orphaned, init-parented)

Both were leftover from past automated browser sessions whose driver
process exited without killing its Chrome subprocess. Chrome's
`EventSource` auto-reconnected through every prior phoenix bounce, so
the streams persisted across many deploys.

These leaks didn't cause the user's wedge directly (different origin —
they connect via `localhost`, the user via VPN), but they:

1. Drain server FDs over time (we saw ~5 stale localhost connections)
2. Could mask real saturation when investigating
3. Indicate a class of bug we have no signal for today

## Root cause

The driver-side cleanup is upstream of phoenix and not actionable
here. But phoenix has zero visibility into "who has SSE streams open
and for how long" — by the time symptoms surface, the only tool is
`ss` and process-tree archaeology. That's how this incident played
out.

## Fix options (pick one or both)

**A. Periodic warn log**
Walk active SSE streams; log at WARN when any peer holds ≥3
concurrent streams or has held a single stream ≥1h. Cheap,
zero new endpoints.

**B. Admin endpoint** (`GET /api/admin/sse-connections`)
Returns peer addr, conv id, stream age, last event sent. Shows up
in a future debug UI. More flexible but more surface area.

A is enough for a single-user dev tool; B becomes useful if phoenix
ever serves more than one human.

## Done When

- [ ] Active SSE streams are tracked with peer + start time
- [ ] Threshold-triggered WARN log fires for per-peer count or per-stream age
- [ ] Manual repro: leak a stream (open then `kill -STOP` the client),
      observe WARN within the threshold window
- [ ] No regression on normal traffic — log volume stays quiet under
      legitimate use

## Notes

Companion to:
- 02704 (bounded-sse-stream-lifetime) — fixes the per-origin
  saturation class. Doesn't catch leaks because EventSource
  reconnects on the lifetime boundary.
- 02703 (shutdown-sse-deadline) — same surface area, different angle.

The actual driver-side fix lives in the agent-browser tool, not this
repo. This task is the phoenix-side observability that would have
turned today's 30-minute investigation into a 30-second log line.
