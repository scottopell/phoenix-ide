# ProductConversation deterministic fixture QA

## Scope

This task slice covers only deterministic Ladle fixture/capture presentation for ProductConversation.
It does **not** modify production ProductConversation, Coordinator, or recovery behavior.

## Fixture surfaces

### ProductConversation

Stable ready marker:

- `data-product-conversation-fixture-ready="<scenario-id>"`

Stable semantic attributes emitted by the fixture shell:

- `data-product-conversation-surface="product-conversation"`
- `data-product-conversation-scenario="<scenario-id>"`
- `data-product-conversation-viewport="desktop|mobile"`
- `data-product-conversation-state="ready|loading|error"`

The real shipped `ProductConversationPage` remains the rendered surface. Readiness waits for the matching title, canonical route, and visible metadata section. Focused fixture assertions separately lock lifecycle, writable-row, and handoff cardinality so the capture marker cannot redefine production state.

Scenarios:

- `desktop-open-multi-segment-qa-work`
- `mobile-open`
- `desktop-history-read-only`
- `mobile-history-read-only`
- `loading`
- `error`
- `long-history-110-messages`

### Coordinator separation

Coordinator remains a separate fixture surface and must not be classified as ProductConversation Open/History.
The fixture exports `COORDINATOR_FIXTURE_SURFACE = "coordinator"` for separation assertions.

### Recovery staging via shipped NewConversation presentation only

Fixture-local recovery staging is allowed only through shipped `NewConversationPage` presentation and shipped generated recovery row/action types.
This slice stages recovery rows without inventing new lifecycle, persistence, or mutation behavior.

Stable ready marker:

- `data-new-conversation-fixture-ready="<scenario-id>"`

Recovery staging scenario:

- `recovery-staging`

## Exact handoff proof

The deterministic handoff summary is:

> Approved handoff: keep exactly one persisted handoff summary between predecessor history and the successor transcript.

The deterministic distinct successor message is:

> Successor kickoff: begin implementation without repeating the persisted handoff summary.

Fixture tests assert that the snapshot contains the handoff summary exactly once, the successor's first message is distinct, and the rendered segment navigation contains exactly one handoff boundary.

## Long transcript proof

The `long-history-110-messages` scenario uses 110 messages across multiple segments.
Fixture tests rerender the real page while asserting that both the snapshot and rendered navigation retain one handoff marker. Desktop/mobile captures exercise the long scrolling surface.

## Capture contract

Capture entrypoint:

- `ui/scripts/capture-product-conversation.mjs`

Viewport matrix:

- desktop: `1440x900`
- mobile: `390x844`

Expected console-error allowance:

- scenario `error` may emit the fixture snapshot-load failure text.

## Backend field adapter checklist

Fixtures consume only already-shipped fields from `ProductConversationSnapshotView` and related shipped types:

- `product_conversation_id`
- `canonical_route`
- `requested_transcript_row_id`
- `canonical_root`
- `ordinary_lifecycle`
- `latest_transcript_row_id`
- `writable_transcript_row_id`
- `updated_at`
- `presentation`
- `work_identity`
- `source`
- `chain_qa_compatibility`
- `segments[]`
- `segments[].segment_ordinal`
- `segments[].transcript_row_id`
- `segments[].title`
- `segments[].messages[]`
- `segments[].handoff`
- `handoff.predecessor_transcript_row_id`
- `handoff.successor_transcript_row_id`
- `handoff.continuation_message_id`
- `handoff.summary`
- `before`
- `has_older`

Recovery staging consumes only already-shipped recovery fields/types:

- `ProductConversationCreationRecoveryRow`
- `request_id`
- `cwd`
- `objective`
- `model`
- `effort`
- `status`
- `updated_at`
- `published_product_conversation_id`
- `llm_language`
- `images`
- `allowed_actions`
- `last_error`
- `ProductConversationCreationAllowedActionView`

## Real isolated-instance journeys

Run `./dev.py qa product-conversation-journeys`. The QA-only harness starts a fresh Phoenix process, database, HOME/config/data directories, Git repository, worktree allocation, and port for each journey. It drives shipped HTTP/SSE contracts and currently proves:

- create with the exact initial objective;
- exactly one aggregate/list row and stable canonical identity after reload;
- deterministic natural context exhaustion followed by a successor transcript, exactly one typed handoff, one aggregate/list row, and latest-successor writable ownership via the merged `product_conversation_context_continuation` scenario;
- clean Close to History;
- busy Close stop-work confirmation;
- dirty-worktree Close with the exact persisted loss identity and confirmation.
- creation delivery failure/retry through the merged typed-capacity integration seam with stable durable identities and global uniqueness.

The creation-failure/retry journey invokes the merged `explicit_retry_after_queue_full_reuses_published_identities_without_duplicate_aggregate` selector. That integration test drives the real creation worker/runtime manager through a legitimate typed steering-capacity admission failure, durable `delivery_failed`, explicit retry after capacity release, stable published product/transcript/message identities, and global one-job/one-publication counts. It remains test-only rather than exposing a production fault endpoint.

No fixture data or direct database mutation substitutes for these user journeys.

## Validation for this slice

- focused Vitest on touched fixture and capture tests
- UI typecheck
- deterministic Ladle capture via the shared capture engine
