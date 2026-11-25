# Keep Coordinator tracking readable across origin-kind changes

## Failure

Production `tracking read` rejected stored `coordinator_question_reply` as an unknown variant. The item could still be selected by `requested_item_due_for_review`, but Ember could not read its evidence to reconcile it.

On this checkout, the scanner already accepts a user-role `InputOrigin::CoordinatorQuestionReply` as primary evidence. `apply.rs` extracts the kind from serialized `InputOrigin`, and migration 168 widened the scanner evidence table's CHECK to admit it. `read_coordinator_tracking_page` decodes that column through the older, independently maintained `ScanOriginKind` and fails the whole detail page. The evidence table's closed list is a second place the next variant could drift.

## Changes

- Share a canonical typed, payload-free origin kind between `InputOrigin`, scanner evidence writes, and tracking provenance reads. Project `InputOrigin` through a compiler-checked exhaustive conversion (or derive the projection); remove the scanner's independent list and JSON discriminator extraction. Preserve existing kind strings and source payload/admission rules. Adding an `InputOrigin` variant must require a corresponding kind at compile time, not depend on remembering a scanner edit.
- Add a forward migration for `coordinator_scan_evidence_origins` that preserves every row and its raw kind, role, locator and attempt, as well as its keys and other constraints. Keep a text-shape check for the provenance label rather than an independently maintained SQL list: the typed application writer supplies known labels, while this historical evidence table can retain unrecognized ones. Do not edit migrations 166/168 or relax constraints on authoritative input tables.
- Decode unrecognized stored scanner kinds at the tracking read boundary as explicitly untrusted provenance (either `unknown_historical` or a distinct unrecognized value); log the fallback without rewriting the row. Preserve the evidence, quote, locator and pagination. Keep unrelated decoding and admission strict. A fallback is not review, permission, or proof of human authorship; it must never feed back into scanner source or input authority.
- Update scanner/tracking requirements, relevant Allium guidance and executive coverage. Record the scoped provenance read/DB decision in an ADR without promising binary downgrade, mixed-version access or general corrupt-DB recovery. Regenerate types via `./dev.py codegen` only if the change affects generated contracts.

## Acceptance evidence

- Persist a typed Coordinator question reply as message source, capture through the scanner, and read exact `coordinator_question_reply` provenance in the tracking detail/tool JSON. Verify the capture is still unreviewed and cover the affected due-cue path through successful detail inspection and normal explicit reconciliation.
- Exercise every current origin kind: the shared projection, existing serde/DB strings, scanner evidence storage and tracking decode agree. Keep the compiler check as the primary guard for added variants.
- On the migrated schema, store a future kind on user-role scanner evidence and verify tracking still returns the item, evidence and locator, with an untrusted fallback and the original DB label untouched. Verify recognized neighboring evidence still decodes normally and unrelated validation errors are not swallowed.
- Test a pre-fix database with existing scanner provenance (including the reply kind admitted by migration 168) and a fresh database. Check retained rows and foreign keys.
- Pass the required hosted checks and independent review on the PR. Do not modify Coordinator system prompts or UX error reporting. No merge, release, or deploy.

## Verification

- [PR #1182](https://github.com/scottopell/phoenix-ide/pull/1182), reviewed implementation head `f72f2e7af90893e3e17b07dc76752a7c5925d160`. All 97 focused origin, scanner, and migration tests passed. The tracking tool serializes the tested detail without reshaping it.
- [Hosted CI attempt 3](https://github.com/scottopell/phoenix-ide/actions/runs/45655169097/attempts/3) passed every required check, including all four Rust test shards, clippy, specs, task validation, and e2e. The non-required macOS release-helper job was cancelled, so the workflow did not report overall success.
- Independent adversarial review and [Codex review](https://github.com/scottopell/phoenix-ide/pull/1182#issuecomment-7168564917) had no actionable findings on the implementation head after the SQLite edge cases and migration assertions were fixed. Final PR head evidence is recorded on the PR.
