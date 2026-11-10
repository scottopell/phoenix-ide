# Native federated conversation create and rename

Implement `create_conversation` and `rename_conversation` with the shared optional destination selector used by other federation-aware coordinator tools. Omitted/local destinations preserve native local behavior; explicit remote destinations use authenticated peer routes with no fallback.

Align federation requirements, Allium, ADR history, and executive status first. Both local and remote adapters must invoke shared typed application actions. Destination validation remains authoritative, mutation retries are idempotent, wire responses are bounded, and durable remote attribution must not fabricate destination-local source transcripts.

## Acceptance criteria

- [ ] Federation and feature-specific normative contracts agree on remote create/rename.
- [ ] Create and rename use shared typed application actions for local HTTP, coordinator tools, and federation destination handlers, with structurally distinct local-user, locally validated tool-call, and authenticated-federated source-authority variants.
- [ ] Selector semantics preserve omission, exact `"local"`, and each existing tool's explicit-`null` compatibility while explicit remote routing has no local fallback and rejects mutation self-routing.
- [ ] Destination authentication/authorization, idempotency, replay/conflict behavior, provenance, and compatibility behavior are explicit and covered.
- [ ] Create success is accepted-only and closes over durable destination acceptance; uncertain remote create uses a strict destination-scoped request-ID status read returning `unknown` or one complete provisioning/publication/route/execution snapshot only to the authenticated acceptance owner, without local or alternate-peer fallback.
- [ ] Source-side dispatch validates current-Coordinator provenance; destination admission authenticates and authorizes the peer without resolving foreign source locators as destination-local authority.
- [ ] Native rename rejects non-ordinary targets; applied and replayed responses expose one authoritative operation/target/root identity plus the immutable normalized title, expected revision, committed slug/timestamp, and first accepted peer-qualified source call needed to classify later replay or changed-intent reuse after rename or deletion.
- [ ] The peer-qualified provenance migration enforces complete source identity, conditionally backfills internal-native creation jobs and retained rename receipts with complete pre-feature locators, preserves provenance-absent creation rows, and round-trips authenticated source-instance plus exact source product/transcript/message/tool-use identifiers through the retained creation acceptance record and immutable rename receipt.
- [ ] Focused regressions cover local/remote routing, explicit-null and contradictory selectors, replay, changed-owner and changed-intent conflicts, non-ordinary rename targets, uncertain-create status ownership/readback, validation, lifecycle failures, unsupported endpoints, and the 65,536-byte create-request, 4,096-byte create-response/status-request, and 16,384-byte create-status-response/rename request/response limits.
- [ ] Allowed local policy/format/task checks pass; hosted CI qualifies the exact pushed head.
