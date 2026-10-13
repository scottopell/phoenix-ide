# Conversation-view telemetry — executive status

Task 32005 is user-commissioned, independently owned telemetry work. Global retains merge/deploy admission; this is not an implicit release blocker.

## Implemented source increment

- `ConversationViewJourney`: first-once monotonic marks, signed readiness difference, null missing marks, canceled/failed/incomplete outcomes, censored five-minute saturation, sixteen observed correlations and drop counters, background duration.
- `ViewJourneyReporter`: admission-time 25% sampling, independent journey UUIDs, per-reporter denominators, one keepalive request with a three-second abort deadline; busy reports drop, with no retry/offline queue.
- `view_journey::report`: authenticated existing API routing, eight-KiB body limit, fixed schema/enums/UUIDs and duration validation, backend source identity, allowlisted `browser.conversation_view` and bounded `browser.view_correlation` exporter spans.
- API requirements explicitly label legacy `first_paint_ms` as a frame scheduling estimate, not compositor proof.

## Verification and remaining qualification

| Requirement | Evidence | Remaining |
|---|---|---|
| REQ-CVT-001 | Pure admission/outcome/unique-ID tests | Route, reload, Back/Forward and PWA-resume hook integration |
| REQ-CVT-002 | Injected network/UI delay, reordered/missing marks, background and saturation tests | Actual aggregate/chapters/breadcrumb/anchor observations |
| REQ-CVT-003 | Privacy-projection and correlation-cap tests; strict collector source | Backend compilation/tests and real trace retrieval |
| REQ-CVT-004 | Fake-timer abort/offline/drop tests; no queue | Real-browser overhead and suspension qualification |
| REQ-CVT-005 | Focused UI tests, narrow TypeScript and lint | Continuation/deep-link lifecycle tests; exact-head hosted CI and independent cumulative review; Global and ordinary candidate correlated traces |

Independent bounded source review found a mutable identity privacy bypass; the final serialization boundary now re-projects and sanitizes identity, with a regression. No Rust/server build has been admitted or run on this host. No production collector/configuration, retention, credential or deployment changes were made. No historical phone incident reconstruction is claimed.

Shared page/anchor changes await the history owner's concrete minimal partition. Existing ordinary telemetry suppression on Global is untouched. Attribution expansion is evidence-led, not included merely because the pipeline exists.
