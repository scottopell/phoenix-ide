# Actual reporter/pagehide probe — observed positive-gate failure

Exact sanitized artifact **12144581523**, job **117738086173**, run
**39148822485** is retained as `result.json`; `provenance.json` records its byte
checksum and source/bundle identities. All four cases completed with no retained
primary or cleanup error. The positive functional gate correctly returned false.

Normal and deferred cases captured the actual incomplete reporter payload; the
synthetic collector received matching complete bodies and attempted/finished 204
responses. Neither page nor context observed a matching client 204; both observed
an exact-request failure. These are separate facts: server finish is not browser
receipt, and request failure does not identify a network abort or undo server body
receipt. Never-204 received a body without sending 204. Unsampled is admission-only,
not pagehide coverage. The pinned terminal qualifier remained provisional when
its status list was empty; this is not a completed-delivery clearance.

## Source-only interpretation and remaining boundary

The production collector handler `api/view_journey.rs` emits
`conversation_view_accepted` with status 204 after its validated report handling,
then returns `StatusCode::NO_CONTENT`. The existing Python
`collector_acceptances` validates that server-side record and identity. The
`validate_collector_export_join` path separately requires exact report/journey,
collector-accepted and actually retrieved export UUID inventories, outcome/trace
matching, no browser failure, and normal drain. Browser response observation is
not a substitute for those server/export receipts, nor are synthetic response
finish events production acceptance records.

Thus this probe demonstrates a forward **observation/qualification boundary**:
actual pagehide reporting can reach a synthetic receiver and finish a server
response while the departing-document client does not observe its 204. It does
not establish a production collector/export invariant violation. It also does not
identify why the earlier stock run failed, prove an abort cause, or locate its
lost primary/cleanup-only assertion. No Phoenix joint integration or ready-report
receipt was manufactured. Historical cause remains open.

No gate relaxation, application change, module-pin update, live fix, second probe,
natural benchmark, cohort pooling or performance claim follows from this result.
The stronger positive gate remains false. Further changes require a specific
contract decision and review, not an automatic retry.
