# ADR-266: Preserve settled text-block starts with an ephemeral content witness

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-VT-001, REQ-VT-004, REQ-VT-010, REQ-VT-014; `VirtualTranscriptPhysicalAuthority`

## Context

A reader can stop inside a tall transcript row while naturally sized media above the visible text finishes loading. Preserving the row's viewport offset alone does not preserve the text's position within that row. The user chose natural sizing rather than a fixed media box. The existing physical authority already combines measured row layout, header extent, and gesture spacer compensation; adding a second scroll owner would conflict with that model.

The candidate needs a bounded settled-reader remedy, not arbitrary glyph tracking or a new history-anchor protocol. Existing command-owned and settled directional geometry, including the lifetime distinction in ADR-120, must retain precedence.

## Options considered

1. **Fixed media boxes** — avoid some late displacement by constraining presentation, but impose sizing the user explicitly declined and do not address all intra-row growth.
2. **Row anchoring alone** — keeps the simplest model and remains a necessary fallback, but cannot preserve visible text when content above it grows inside the same row.
3. **Ephemeral text-block witness in the physical authority** — retains natural sizing and captures a useful pre-change location, at the cost of bounded DOM inspection and strict witness eligibility and retirement.
4. **Independent content scroll correction or arbitrary glyph/history tracking** — could pursue a wider continuity promise, but adds competing compensation or identity machinery beyond the requested settled-reader case.

## Decision

Use an ephemeral witness to a surviving text-block start within the top visible row. Acquire only while the reader is passively settled and no command, settled directional anchor, pending positioning, or tail execution has precedence. Search a bounded set of candidates, accepting a nonempty positive-height text block only when its start is inside the viewport.

Retain the mounted row and element identity with the pre-change viewport offset. On measured change, derive the effective row anchor offset from that captured viewport offset minus the element's new row-local offset. Pass this through the existing anchor compensation path so preceding-row movement and intra-row displacement produce one combined target, not successive competing corrections.

Retire on gesture/scroll, navigation, tail positioning, view change, or detach. Respect the existing bounded stale-touch semantics when acquiring again. If the witness is unavailable, removed, or replaced, retain ordinary row anchoring rather than inventing pre-change geometry from a post-change measurement.

## Consequences

- **Positive:** Naturally sized content can change above stable visible text without introducing another viewport owner or height model.
- **Negative:** DOM identity and a pre-change capture are required. Bounded search may find no eligible block, and native scroll bounds can limit correction. Reflow within a block, replaced text identity, and content-level history restoration are outside the promise.
- **Neutral:** Existing row anchoring, gesture spacer compensation, command positioning, and tail policy remain authoritative in their respective cases. A settle timer permits capture; it does not establish positioning success.
- **Verification:** This decision records the candidate design, not completed tests, CI, deployment, or physical Safari acceptance. Those require separate evidence.

## References

- [Virtual Transcript requirements](../virtual-transcript/requirements.md), especially REQ-VT-014.
- [Virtual Transcript behavioral contract](../virtual-transcript/virtual_transcript.allium).
- [ADR-120](120_successful-position-settles-geometric-anchor.md).
- `ui/src/components/VirtualTranscript.tsx`: `ContentWitness`, `witnessEligible`, `acquireContentWitness`, `witnessLocalOffset`, `applyAnchor`, `scheduleWitness`.
