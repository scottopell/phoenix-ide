# Ember phase 4: tracking continuity and inspection

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Depends on phase 3. This slice proves the feature helps after continuation before adding a model scanner.

## Outcome

Ember sees a fresh compact index of what it is tracking on subsequent dispatches and continuations, and the user can inspect the saved record and evidence without maintaining a second project-management system.

## Work

- Add a bounded server-rendered tracking projection at the existing fresh Coordinator-context construction boundary. Include registry revision, active count, entries/read pointers and explicit partial-list/pagination information.
- Keep full notes/evidence behind bounded native reads. Never present a prefix as the complete or fully reviewed list. Define ordering and overflow behavior; no invented priority scores.
- Use stable singleton ownership through continuation topology. Exact source citations remain pinned; a current conversation link resolves its actual successor.
- Treat notes and extracted source as untrusted stored data. Do not promote prose into Authority or reported live state. Clearly separate a recorded promise from permission to act.
- Update global-recall requirements/ADRs for the deliberate new memory projection. Preserve the decision to fetch live activity on demand rather than reviving an inferred global activity capsule.
- Keep /global chat-first. Establish user inspectability via explicit read/list and rendered tool/conversation results. Any bounded list/detail control must name its web/native scope and validation; no new workstream navigation identity or dashboard.
- Distinguish record existence, last explicit review and scanner coverage. A successful scan does not mean Ember reviewed or completed the obligation. The scanner may add while idle without waking work.
- Follow Issue #1075's thread-keeping behavior: retain user-requested work and useful question bookmarks, answer the current question first, avoid repeating the list each response, and provide a short refresher/resumption point when the user returns. Topic change alone neither abandons prior work nor commissions a new task. Automatic revisit/wake policy is still under product discussion; do not infer it merely from saving a record.

## Starting points

runtime/executor.rs Coordinator prompt construction near build_coordinator_system_prompt; system_prompt.rs; runtime/executor/continuation.rs; specs/global-recall/requirements.md; coordinator_tools registration. For presentation inspect ui/src Coordinator/shared conversation tool-result rendering and ios client contracts before proposing extra UI.

## Done when

A record created before continuation is discoverable in the successor's fresh context without relying on the handoff summary; retirement/revisions survive restart; overflow is explicit; exact historical evidence and current conversation navigation remain distinct. Test malicious note text, absent evidence, ordinary/child context exclusion, empty registry and over-budget index.

Qualify through production context construction and a controlled real continuation journey. Existing four-tool coordinator-drive-eval does not prove this path. Run owning Rust/spec checks and UI/native checks only for actual presentation changes.
