# Compact expanded-tool chronology/navigation

Incident: GitHub issue comment 5668942047 reports a mobile-browser transcript UX failure where compact mode expansion of an older tool call can confuse chronology/navigation as newer activity arrives.

Acceptance:

- Reproduce the actual viewport sequence: compact mode, expand older tool A, append B/C, complete, then final message.
- Determine and document the authoritative message order before assigning blame: persisted order → `buildRenderUnits` compact partition → grouped/expanded placement and keys → virtual extent/anchoring/follow intent.
- Latest activity remains reachable throughout the sequence.
- Chronology and tool-result association remain clear when an older tool is expanded while newer tool activity and final prose arrive.
- Intentional old reading is preserved; the user is not forced to the tail merely because B/C/final append.
- Collapse and jump-to-latest recover the tail without remounting the conversation or losing compact expansion state unnecessarily.
- Regression coverage includes narrow WebKit and desktop, streaming and finalized paths, resize behavior, grouped tool-only turns, and virtualized transcript scroll ownership.

Non-goals:

- Do not blame or replace the virtualizer until authoritative order, partitioning, expansion placement/keys, extent updates, anchoring, and follow intent have been traced.
- Do not merge/deploy/native-iOS/Crick/lifecycle changes as part of this UX fix.
