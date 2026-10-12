# ADR-093: Sampled live-job resource visibility, not exhaustive accounting

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-WSLOAD-001, REQ-WSLOAD-002, REQ-WSLOAD-005

## Context

Hosted review identified that endpoint native CPU snapshots cannot detect a child that starts and exits entirely between observations, and tracked handle terminalization ends enumeration even if an operating-system group survives. Registry lifecycle epochs detect tracked-job changes, not all native child activity. Calling measured duration completeness overstated the guarantees.

The user explicitly selected "SHIP SAMPLED VISIBILITY": lightweight CPU/memory indicators for live tracked jobs, not exhaustive accounting; short-lived children may be missed; attribution ends with the tracked handle; sampling duration is not completeness.

## Options considered

1. Add durable group CPU accounting and native child lifecycle tracking, with ownership continuation after handle termination.
2. Ship bounded sampled visibility, retain concrete freshness/identity/membership/lifecycle race defenses, and expose the measurement limits consistently.

## Decision

Choose option 2. API windows use `sampled_seconds`, and summaries expose a typed `live_tracked_bash_groups` measurement basis. Tool and inline UI describe sampled observed counters, missed short-lived children and tracking ending with the handle. Zero current values mean no tracked live jobs, not host inactivity or an exhaustive scope footprint.

Concrete races remain errors: observed membership/identity changes, tracked-job lifecycle changes, and read failures invalidate measurements. Memory requires stable pre/post observations. UI freshness uses monotonic receipt time and server-domain CPU age. These fixes are retained, not waived by sampling caveats.

## Consequences

- The feature stays efficient and bounded without a monitoring framework or native descendant tracker.
- Sampled duration describes the time represented by accepted samples; it does not certify all CPU used by native children was captured.
- Groups whose leader proof is missing remain unavailable while the handle is live; after handle terminalization they are outside the live-tracked-job model.
- Exhaustive group accounting and post-handle survivor diagnostics are not promised by this feature.
