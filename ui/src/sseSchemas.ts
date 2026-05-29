// SSE wire-format runtime validation (task 02674) + compile-time
// alignment to Rust-generated types (task 02677).
//
// Every SSE event handler in `hooks/useConnection.ts` used to cast the result
// of `JSON.parse` with `as SomeType`, which the TypeScript compiler enforces
// at compile time and *nothing* enforces at runtime. A malformed-but-parseable
// payload (missing field, wrong type, null where a string was expected) would
// reach the conversation reducer unchanged and silently corrupt state — most
// dangerously by letting a string `sequence_id` through, which breaks the
// `atom.lastSequenceId >= action.sequenceId` dedup guard via string compare.
//
// As of task 02677 the schemas are typed with `v.GenericSchema<T>` where `T`
// is the Rust-generated wire type from `./generated/sse`. That closes the
// loop: a Rust type change bubbles up as a regenerated TS type, which tsc
// then rejects against the valibot schema until the schema is updated to
// match. Drift between the Rust wire format and the runtime validator is
// now a compile error rather than a production runtime surprise.
//
// Strictness: object schemas are *loose* (extra top-level keys allowed). The
// Rust backend adds forward-compatible fields routinely; rejecting unknown
// keys would turn every minor server addition into a client-side crash, which
// is worse than the original silent-drift problem. What we validate is that
// required fields are present and typed correctly.

import * as v from 'valibot';
import type { ErrorKind as WireErrorKind } from './generated/sse';
import type { Conversation, Message } from './api';
// Generated wire types — aliased so we can reuse the short `Sse*Data`
// names for the transform-output types consumers actually want.
import type {
  SseInitData as WireInitData,
  SseMessageData as WireMessageData,
  SseMessageUpdatedData as WireMessageUpdatedData,
  SseStateChangeData as WireStateChangeData,
  SseTokenData as WireTokenData,
  SseLlmFirstByteData as WireLlmFirstByteData,
  SseLlmAttemptData as WireLlmAttemptData,
  SseAgentDoneData as WireAgentDoneData,
  SseConversationBecameTerminalData as WireConversationBecameTerminalData,
  SseConversationUpdateData as WireConversationUpdateData,
  SseErrorData as WireErrorData,
  SseConversationHardDeletedData as WireConversationHardDeletedData,
  SseBrowserSessionStateData as WireBrowserSessionStateData,
  SseSteerMessageQueuedData as WireSteerMessageQueuedData,
  ErrorPresentation as WireErrorPresentation,
  SseRateLimitSnapshotData as WireRateLimitSnapshotData,
  QuotaDetails as WireQuotaDetails,
  RateLimitWindow as WireRateLimitWindow,
  CreditsSnapshot as WireCreditsSnapshot,
  SseBreadcrumb as GeneratedSseBreadcrumb,
  ChainQaTokenData as WireChainQaTokenData,
  ChainQaCompletedData as WireChainQaCompletedData,
  ChainQaFailedData as WireChainQaFailedData,
  BashResponse as WireBashResponse,
  BashErrorResponse as WireBashErrorResponse,
  TmuxToolResponse as WireTmuxToolResponse,
  TmuxErrorResponse as WireTmuxErrorResponse,
} from './generated/sse';

// ---------------------------------------------------------------------------
// Supporting schemas for objects reused across event envelopes. These
// validate the load-bearing fields (the ones where a type drift silently
// corrupts the reducer — `sequence_id` is the exemplar) and accept the full
// richer shape as trusted wire data once the critical fields pass.
//
// The `v.pipe(..., v.transform())` pattern below is the explicit seam where
// we move from "wire view validated by the schema" to "domain type consumed
// by the UI". Casting here keeps the trust boundary visible in one file
// instead of scattering `as Message` casts at every consumer.
// ---------------------------------------------------------------------------

/** Conversation object as it arrives on the wire. UI consumers render many
 *  optional fields; we validate only `id` (the stable identifier every caller
 *  depends on) and trust the rest — the Rust backend's serde-serialized
 *  `EnrichedConversation` is the structural source of truth for optional
 *  fields. The generated `SseInitData.conversation` is `unknown` on purpose
 *  (the full Conversation shape is hand-authored in `./api.ts` and the
 *  generated wire type avoids duplicating it); the transform below is the
 *  single boundary where we cast to the rich `Conversation` type. */
const ConversationSchema = v.pipe(
  v.looseObject({ id: v.string() }),
  v.transform((obj): Conversation => obj as unknown as Conversation),
);

/** Hand-authored mirror of the Rust `MessageType` enum
 *  (see `ui/src/generated/MessageType.ts`). The picklist is strict so an
 *  unknown type surfaces as a schema violation (forward-compat risk accepted
 *  for this field — new message types are rare and additive). A conversation's
 *  history can include `error` messages (parse-error fallback) and
 *  `continuation` messages (continuation summaries), so both must be listed
 *  here — otherwise init for any conversation with those in history would
 *  fail to validate.
 *
 *  Exported for a tripwire test in `sseSchemas.test.ts` that asserts this
 *  list matches the generated `MessageType` union character-for-character.
 *  Without the tripwire, a new Rust-side variant would fail only at runtime
 *  (parse violation → toast) the first time a conversation carrying the new
 *  type hit the client. `satisfies` on the schema below catches schemas that
 *  are narrower than the wire type, but NOT a hand-authored picklist whose
 *  narrowness is the intent. */
export const MESSAGE_TYPE_OPTIONS = [
  'user',
  'agent',
  'tool',
  'system',
  'skill',
  'error',
  'continuation',
] as const;

/** Message block carried in `init.messages` and `message.message`. Validates
 *  the reducer's load-bearing fields (`sequence_id` as number is the main
 *  point — a string would corrupt the dedup guard).
 *
 *  `content` is a discriminated union (text / content-blocks / tool-result)
 *  already tolerated by the reducer and view layer — we don't re-derive that
 *  union here because the server's Rust enum is the source of truth and
 *  duplicating it in valibot would create parallel representations. */
const MessageSchema = v.pipe(
  v.looseObject({
    message_id: v.string(),
    sequence_id: v.number(),
    conversation_id: v.string(),
    message_type: v.picklist(MESSAGE_TYPE_OPTIONS),
    content: v.unknown(),
    display_data: v.optional(v.unknown()),
    usage_data: v.optional(v.unknown()),
    created_at: v.string(),
  }),
  v.transform((obj): Message => obj as unknown as Message),
);

/** Breadcrumb as it appears on the wire (snake_case) before the UI transform.
 *
 *  The schema is stricter than the generated `SseBreadcrumb` type (which has
 *  `type: string` because the Rust field is a `String`, not an enum). We
 *  intentionally enforce the closed `picklist` here — the set of breadcrumb
 *  kinds is small, stable, and UI code does symbol-style comparisons on it.
 *  A Rust-side change that introduces a new crumb type would fail at runtime
 *  in prod (toast via `sse_error`) until this list is updated.
 *
 *  `v.exactOptional` (rather than `v.optional`) lines up with ts-rs'
 *  `#[ts(optional)]` emission — with `exactOptionalPropertyTypes: true`
 *  in tsconfig, `field?: T` forbids an explicit `undefined` value. The
 *  Rust wire uses `skip_serializing_if = "Option::is_none"`, so `None`
 *  means "key absent", not "key = undefined". */
const SseBreadcrumbSchema = v.looseObject({
  type: v.picklist(['user', 'llm', 'tool', 'subagents']),
  label: v.string(),
  tool_id: v.exactOptional(v.string()),
  sequence_id: v.exactOptional(v.number()),
  preview: v.exactOptional(v.string()),
}) satisfies v.GenericSchema<unknown, GeneratedSseBreadcrumb>;
export type SseBreadcrumb = v.InferOutput<typeof SseBreadcrumbSchema>;

// ---------------------------------------------------------------------------
// Event schemas. One per `addEventListener` in useConnection.ts.
//
// Each `SseXxxDataSchema` is annotated with `v.GenericSchema<SseXxxData>`,
// where `SseXxxData` comes from `./generated/sse`. TSC rejects at compile
// time if the schema's InferOutput drifts from the Rust-derived type.
// ---------------------------------------------------------------------------

// Every event schema below carries `sequence_id: v.number()` as a required
// field (task 02675). The client's reducer routes every action through a
// single `applyIfNewer(atom, sequence_id, apply)` helper, so a missing or
// string-typed sequence_id is a protocol violation the schema must reject —
// not something we want to quietly tolerate and then crash in the reducer.

/** `init`: full state snapshot at connect / reconnect.
 *
 *  `conversation`, `messages`, `breadcrumbs` are the structured fields the
 *  reducer reads. `project_name` is a top-level mirror that
 *  `transformInitData` in useConnection.ts merges back into the conversation
 *  object — it lives at the top level on the wire because the Rust
 *  `SseEvent::Init` struct carries it as a flat field.
 *
 *  `sequence_id` and `last_sequence_id` are the same number by construction
 *  (the snapshot's own place in the total order equals the highest-ever-
 *  emitted id at subscribe time). Both are required.
 *
 *  `presentation_mode` is `string` (not optional) in the Rust wire type — task
 *  02677 tightened this field from the previously-optional schema shape
 *  after the generated type surfaced the actual wire contract. */
export const SseInitDataSchema = v.looseObject({
  sequence_id: v.number(),
  conversation: ConversationSchema,
  messages: v.array(MessageSchema),
  agent_working: v.boolean(),
  last_sequence_id: v.number(),
  presentation_mode: v.string(),
  context_window_size: v.number(),
  breadcrumbs: v.array(SseBreadcrumbSchema),
  project_name: v.nullable(v.string()),
  // ReplayRing snapshot (Phase 2: server-side wiring). The reducer does
  // not yet consume these — see `specs/sse_wire/sse_wire.allium`
  // StreamOpened and `tasks/62002` for Phase 3 client wiring. Validate
  // the load-bearing top-level shape now so a future reducer addition
  // can rely on the fields being present and well-typed.
  //
  // `pending_events` is intentionally `unknown[]`: Phase 3 will apply
  // the per-event schemas (SseTokenDataSchema etc.) at replay time so
  // each entry is validated through the same path as its live
  // counterpart. Validating it here as a strongly-typed discriminated
  // union would require a recursive SseWireEvent schema that drifts
  // independently from the per-event validators below.
  pending_anchor_sequence_id: v.number(),
  pending_events: v.array(v.unknown()),
  pending_truncated: v.boolean(),
}) satisfies v.GenericSchema<unknown, WireInitData>;

/** `message`: a newly-created message joins the conversation.
 *
 *  The envelope `sequence_id` is the same integer as `message.sequence_id`
 *  by construction (server guarantees equality — see `SseBroadcaster::send_message`
 *  in `src/runtime.rs`). The reducer uses the envelope id for ordering and
 *  the message id for identity-based defense-in-depth dedup. */
export const SseMessageDataSchema = v.looseObject({
  sequence_id: v.number(),
  message: MessageSchema,
}) satisfies v.GenericSchema<unknown, WireMessageData>;

/** `message_updated`: in-place mutation of an existing message's mutable
 *  fields. `display_data` and `content` are optional because either one can
 *  change independently — the server sends both keys (possibly `null`) every
 *  time. `duration_ms` is present only on tool-result updates; absent on
 *  all other paths. `sequence_id` is the envelope id; the persisted
 *  `message.sequence_id` is immutable and not repeated here. */
export const SseMessageUpdatedDataSchema = v.looseObject({
  sequence_id: v.number(),
  message_id: v.string(),
  display_data: v.nullable(v.unknown()),
  content: v.nullable(v.unknown()),
  duration_ms: v.exactOptional(v.number()),
}) satisfies v.GenericSchema<unknown, WireMessageUpdatedData>;

const ERROR_KIND_OPTIONS = [
  'auth',
  'rate_limit',
  'usage_limit_reached',
  'network',
  'invalid_request',
  'server_error',
  'server_overloaded',
  'timed_out',
  'cancelled',
  'sub_agent_error',
  'context_exhausted',
  'content_filter',
] as const satisfies readonly WireErrorKind[];

const ErrorPresentationSchema = v.looseObject({
  kind: v.picklist(ERROR_KIND_OPTIONS),
  can_auto_retry: v.boolean(),
  can_user_resume: v.boolean(),
}) satisfies v.GenericSchema<unknown, WireErrorPresentation>;

/** `state_change`: conversation phase transition. The inner `state` is a
 *  discriminated union by `type` (idle / awaiting_llm / tool_executing / …).
 *  Rather than re-derive that union here, we pass the raw value to
 *  `parseConversationState` in utils.ts, which already performs its own
 *  tagged-union validation. We just assert the envelope is present. */
export const SseStateChangeDataSchema = v.looseObject({
  sequence_id: v.number(),
  state: v.unknown(),
  presentation_mode: v.string(),
  /** RFC3339 string on the wire — the same shape as the Init carrier
   *  (`Conversation.state_updated_at` via `#[serde(flatten)]`). The
   *  SSE-handler boundary converts to ms once via `Date.parse(s)`
   *  before storing on the atom. Specs:
   *  `specs/working-phase-visibility/` REQ-WPV-001. */
  state_updated_at: v.string(),
  error: v.exactOptional(ErrorPresentationSchema),
}) satisfies v.GenericSchema<unknown, WireStateChangeData>;

/** `token`: ephemeral streaming delta during an LLM request. */
export const SseTokenDataSchema = v.looseObject({
  sequence_id: v.number(),
  text: v.string(),
  request_id: v.string(),
}) satisfies v.GenericSchema<unknown, WireTokenData>;

/** `llm_first_byte`: marker emitted exactly once per LLM request,
 *  immediately before the first `Token` event for the same `request_id`.
 *  Drives the StateBar's `awaiting LLM response Ns` → `streaming` transition.
 *  Spec: `specs/working-phase-visibility/` REQ-WPV-007. */
export const SseLlmFirstByteDataSchema = v.looseObject({
  sequence_id: v.number(),
  request_id: v.string(),
}) satisfies v.GenericSchema<unknown, WireLlmFirstByteData>;

/** `llm_attempt`: retry-context marker emitted from the executor's
 *  `Effect::ScheduleRetry` handler immediately before the spawned
 *  backoff sleep. Drives the StateBar's `(retry K/N <reason>)`
 *  suffix per specs/llm-retry-visibility/ REQ-LRV-001 / 003. */
export const SseLlmAttemptDataSchema = v.looseObject({
  sequence_id: v.number(),
  attempt: v.number(),
  max_attempts: v.number(),
  reason: v.picklist(['rate_limit', 'server_error', 'network']),
  backing_off_ms: v.number(),
  /** RFC3339 string when known; omitted from JSON when None on the
   *  Rust side (`skip_serializing_if = "Option::is_none"`). */
  resets_at: v.exactOptional(v.string()),
}) satisfies v.GenericSchema<unknown, WireLlmAttemptData>;

/** `conversation_update`: partial conversation metadata update. The backend
 *  sends a strict subset of the Conversation fields (see Rust
 *  `ConversationMetadataUpdate`). We accept any object and let the reducer
 *  merge it shallowly — forward compatibility matters here more than
 *  enforcement, because new metadata fields are added frequently. */
export const SseConversationUpdateDataSchema = v.looseObject({
  sequence_id: v.number(),
  conversation: v.record(v.string(), v.unknown()),
}) satisfies v.GenericSchema<unknown, WireConversationUpdateData>;

/** `agent_done`: empty envelope apart from the sequence_id. Still validated
 *  so that a future server change that adds fields can be discovered by a
 *  type-check or a new test rather than a silent nop. */
export const SseAgentDoneDataSchema = v.looseObject({
  sequence_id: v.number(),
}) satisfies v.GenericSchema<unknown, WireAgentDoneData>;

/** `conversation_became_terminal`: carries only the sequence_id today.
 *  Wired up as a no-op in the UI but validated so that if the server starts
 *  including teardown detail it is not silently dropped. */
export const SseConversationBecameTerminalDataSchema = v.looseObject({
  sequence_id: v.number(),
}) satisfies v.GenericSchema<unknown, WireConversationBecameTerminalData>;

/** `error` (backend-application channel): distinguished from a native
 *  EventSource connection error (which arrives with no `data` at all) by
 *  the presence of a parseable JSON body. The backend emits a flat
 *  `message` string plus a typed `error` object; the UI has historically
 *  read `message` and we keep that contract while forwarding the typed
 *  error for kind-aware affordances.
 *
 *  Native EventSource connection-reset errors go through a different path
 *  in useConnection.ts and do not use this schema. */
export const SseErrorDataSchema = v.looseObject({
  sequence_id: v.number(),
  message: v.string(),
  error: v.unknown(),
}) satisfies v.GenericSchema<unknown, WireErrorData>;

/** `conversation_hard_deleted`: REQ-BED-032 step 6. Conversation row is gone
 *  from SQLite; all per-conversation resources (bash handles, tmux server,
 *  worktree) have been cleaned up. UI subscribers refresh sidebar /
 *  navigation. The cascade today emits this on the per-conversation
 *  channel only; sidebar listeners that aren't on the deleted conversation
 *  rely on the `DesktopLayout` 5s polling to pick up the deletion. */
export const SseConversationHardDeletedDataSchema = v.looseObject({
  sequence_id: v.number(),
  conversation_id: v.string(),
}) satisfies v.GenericSchema<unknown, WireConversationHardDeletedData>;

/** `browser_session_state`: fired on the server's create / destroy edge for
 *  a browser session in `BrowserSessionManager`. The UI uses this as the
 *  single source of truth for whether to show / auto-mount the live browser
 *  view (REQ-BT-018). Replaces the old client-side proxy that walked
 *  messages looking for `browser_*` tool uses. */
export const SseBrowserSessionStateDataSchema = v.looseObject({
  sequence_id: v.number(),
  active: v.boolean(),
}) satisfies v.GenericSchema<unknown, WireBrowserSessionStateData>;

/** `steer_message_queued`: a steering message was accepted and queued for
 *  delivery when the conversation next reaches `Idle`. The UI uses this to
 *  confirm the client-side `steering_queued` status and surface a queue
 *  position indicator on the message bubble. */
export const SseSteerMessageQueuedDataSchema = v.looseObject({
  sequence_id: v.number(),
  message_id: v.string(),
  queue_position: v.number(),
}) satisfies v.GenericSchema<unknown, WireSteerMessageQueuedData>;

/** Per-window quota state (`primary` / `secondary` slots inside QuotaDetails).
 *  `used_percent` is the only guaranteed field; `window_minutes` and
 *  `resets_at` are absent on minimal codex payloads. */
const RateLimitWindowSchema = v.looseObject({
  used_percent: v.number(),
  window_minutes: v.nullable(v.number()),
  resets_at: v.nullable(v.number()),
}) satisfies v.GenericSchema<unknown, WireRateLimitWindow>;

const CreditsSnapshotSchema = v.looseObject({
  has_credits: v.boolean(),
  unlimited: v.boolean(),
  balance: v.nullable(v.string()),
}) satisfies v.GenericSchema<unknown, WireCreditsSnapshot>;

/** Structured quota state — every field is nullable per the Rust spec
 *  (`crates/phoenix-ide/src/llm/rate_limit.rs`). The SSE-event path
 *  (task 67003) populates `primary` / `secondary` / `credits` /
 *  `plan_type` / `limit_id`; the 429-header path adds `resets_at`,
 *  `limit_name`, `promo_message`. */
const QuotaDetailsSchema = v.looseObject({
  plan_type: v.nullable(v.string()),
  resets_at: v.nullable(v.string()),
  limit_id: v.nullable(v.string()),
  limit_name: v.nullable(v.string()),
  primary: v.nullable(RateLimitWindowSchema),
  secondary: v.nullable(RateLimitWindowSchema),
  credits: v.nullable(CreditsSnapshotSchema),
  promo_message: v.nullable(v.string()),
}) satisfies v.GenericSchema<unknown, WireQuotaDetails>;

/** `rate_limit_snapshot`: mid-stream quota update from the codex backend
 *  (task 67003). Ephemeral — not persisted; the latest snapshot drives
 *  the Settings dropdown's quota row. */
export const SseRateLimitSnapshotDataSchema = v.looseObject({
  sequence_id: v.number(),
  snapshot: QuotaDetailsSchema,
}) satisfies v.GenericSchema<unknown, WireRateLimitSnapshotData>;

// ---------------------------------------------------------------------------
// Chain Q&A wire-event schemas (Phoenix Chains v1, REQ-CHN-004 / 005).
//
// Distinct from the conversation-scoped events above because chain
// broadcasters carry a per-question demux discriminator (`chain_qa_id`)
// instead of the per-conversation monotonic `sequence_id`. Schemas use the
// same `satisfies v.GenericSchema<unknown, T>` annotation pattern so a
// Rust-side change to `ChainSseWireEvent` lights up here as a tsc error
// against the generated TS type.
// ---------------------------------------------------------------------------

/** Streaming token chunk for an in-flight chain Q&A. */
export const ChainQaTokenSchema = v.looseObject({
  chain_qa_id: v.string(),
  delta: v.string(),
}) satisfies v.GenericSchema<unknown, WireChainQaTokenData>;

/** Stream completed cleanly. `full_answer` matches what was just persisted
 *  to `chain_qa.answer`; subsequent reads via the GET endpoint return the
 *  same string. */
export const ChainQaCompletedSchema = v.looseObject({
  chain_qa_id: v.string(),
  full_answer: v.string(),
}) satisfies v.GenericSchema<unknown, WireChainQaCompletedData>;

/** Stream ended in error before producing a full answer. `partial_answer`
 *  carries whatever tokens streamed before the failure (may be `null` when
 *  no token was emitted). */
export const ChainQaFailedSchema = v.looseObject({
  chain_qa_id: v.string(),
  error: v.string(),
  partial_answer: v.nullable(v.string()),
}) satisfies v.GenericSchema<unknown, WireChainQaFailedData>;

export type ChainQaTokenData = v.InferOutput<typeof ChainQaTokenSchema>;
export type ChainQaCompletedData = v.InferOutput<typeof ChainQaCompletedSchema>;
export type ChainQaFailedData = v.InferOutput<typeof ChainQaFailedSchema>;

// The `Sse*Data` types callers import are the schemas' `InferOutput`s —
// i.e. what the validator produces after transforming wire data into UI
// types (Conversation, Message). This is what the reducer and hooks
// actually consume. The schemas' `satisfies v.GenericSchema<unknown, T>`
// annotations bind each schema to its Rust-generated wire shape for
// compile-time drift detection.
export type SseInitData = v.InferOutput<typeof SseInitDataSchema>;
export type SseMessageData = v.InferOutput<typeof SseMessageDataSchema>;
export type SseMessageUpdatedData = v.InferOutput<typeof SseMessageUpdatedDataSchema>;
export type SseStateChangeData = v.InferOutput<typeof SseStateChangeDataSchema>;
export type SseTokenData = v.InferOutput<typeof SseTokenDataSchema>;
export type SseConversationUpdateData = v.InferOutput<typeof SseConversationUpdateDataSchema>;
export type SseAgentDoneData = v.InferOutput<typeof SseAgentDoneDataSchema>;
export type SseConversationBecameTerminalData = v.InferOutput<
  typeof SseConversationBecameTerminalDataSchema
>;
export type SseErrorData = v.InferOutput<typeof SseErrorDataSchema>;
export type SseConversationHardDeletedData = v.InferOutput<
  typeof SseConversationHardDeletedDataSchema
>;
export type SseBrowserSessionStateData = v.InferOutput<
  typeof SseBrowserSessionStateDataSchema
>;
export type SseSteerMessageQueuedData = v.InferOutput<typeof SseSteerMessageQueuedDataSchema>;
export type SseRateLimitSnapshotData = v.InferOutput<typeof SseRateLimitSnapshotDataSchema>;
export type QuotaDetails = v.InferOutput<typeof QuotaDetailsSchema>;
export type RateLimitWindow = v.InferOutput<typeof RateLimitWindowSchema>;
export type CreditsSnapshot = v.InferOutput<typeof CreditsSnapshotSchema>;

// ---------------------------------------------------------------------------
// Bash and tmux tool response schemas (task 02697).
//
// These validate the JSON the tool emits as `tool_result` content (carried
// inside an enriched message's `content` / `display_data` payload). Each
// schema is annotated `satisfies v.GenericSchema<unknown, T>` against the
// Rust-generated wire type so a Rust-side change surfaces as a tsc error
// here.
//
// Object schemas remain `looseObject` — the backend may add forward-compat
// fields (e.g. additional kill metadata); rejecting unknown keys would
// turn every additive Rust change into a runtime crash.
// ---------------------------------------------------------------------------

const BashRingLineSchema = v.looseObject({
  offset: v.number(),
  bytes: v.string(),
});

const BashRingWindowFieldsSchema = {
  start_offset: v.number(),
  end_offset: v.number(),
  truncated_before: v.boolean(),
  lines: v.array(BashRingLineSchema),
} as const;

const BashRunningPayloadSchema = v.looseObject({
  status: v.literal('running'),
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  display: v.string(),
  kill_signal_sent: v.exactOptional(v.nullable(v.string())),
  kill_attempted_at: v.exactOptional(v.nullable(v.string())),
  signal_sent: v.exactOptional(v.nullable(v.string())),
  ...BashRingWindowFieldsSchema,
});

const BashStillRunningPayloadSchema = v.looseObject({
  status: v.literal('still_running'),
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  waited_ms: v.number(),
  kill_signal_sent: v.exactOptional(v.nullable(v.string())),
  kill_attempted_at: v.exactOptional(v.nullable(v.string())),
  ...BashRingWindowFieldsSchema,
});

const BashKillPendingKernelPayloadSchema = v.looseObject({
  status: v.literal('kill_pending_kernel'),
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  kill_signal_sent: v.string(),
  kill_attempted_at: v.string(),
  display: v.exactOptional(v.nullable(v.string())),
  signal_sent: v.exactOptional(v.nullable(v.string())),
  waited_ms: v.exactOptional(v.nullable(v.number())),
  ...BashRingWindowFieldsSchema,
});

const BashTombstonedPayloadSchema = v.looseObject({
  status: v.literal('tombstoned'),
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  final_cause: v.string(),
  exit_code: v.nullable(v.number()),
  signal_number: v.exactOptional(v.nullable(v.number())),
  duration_ms: v.number(),
  finished_at: v.string(),
  kill_signal_sent: v.exactOptional(v.nullable(v.string())),
  kill_attempted_at: v.exactOptional(v.nullable(v.string())),
  display: v.exactOptional(v.nullable(v.string())),
  signal_sent: v.exactOptional(v.nullable(v.string())),
  ...BashRingWindowFieldsSchema,
});

const BashRunTombstonePayloadSchema = (status: 'exited' | 'killed') =>
  v.looseObject({
    status: v.literal(status),
    handle: v.string(),
    cmd: v.string(),
    label: v.exactOptional(v.nullable(v.string())),
    final_cause: v.string(),
    exit_code: v.nullable(v.number()),
    signal_number: v.exactOptional(v.nullable(v.number())),
    duration_ms: v.number(),
    finished_at: v.string(),
    kill_signal_sent: v.exactOptional(v.nullable(v.string())),
    kill_attempted_at: v.exactOptional(v.nullable(v.string())),
    ...BashRingWindowFieldsSchema,
  });

const BashWaiterPanickedPayloadSchema = v.looseObject({
  status: v.literal('waiter_panicked'),
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  error_message: v.string(),
});

/** Discriminated bash response. Branches by the `status` tag (REQ-BASH-002 /
 *  REQ-BASH-003 / REQ-BASH-006). Drift from the Rust wire type surfaces as a
 *  tsc error against the `satisfies` annotation. */
export const BashResponseSchema = v.variant('status', [
  BashRunningPayloadSchema,
  BashStillRunningPayloadSchema,
  BashKillPendingKernelPayloadSchema,
  BashTombstonedPayloadSchema,
  BashRunTombstonePayloadSchema('exited'),
  BashRunTombstonePayloadSchema('killed'),
  BashWaiterPanickedPayloadSchema,
]) satisfies v.GenericSchema<unknown, WireBashResponse>;

const BashLiveHandleSummarySchema = v.looseObject({
  handle: v.string(),
  cmd: v.string(),
  label: v.exactOptional(v.nullable(v.string())),
  age_seconds: v.number(),
  status: v.string(),
});

/** Bash error envelope (REQ-BASH-008). */
export const BashErrorResponseSchema = v.variant('error', [
  v.looseObject({
    error: v.literal('handle_not_found'),
    error_message: v.string(),
    handle_id: v.string(),
    hint: v.string(),
  }),
  v.looseObject({
    error: v.literal('handle_cap_reached'),
    error_message: v.string(),
    cap: v.number(),
    live_handles: v.array(BashLiveHandleSummarySchema),
    hint: v.string(),
  }),
  v.looseObject({
    error: v.literal('wait_seconds_out_of_range'),
    error_message: v.string(),
    provided: v.number(),
    max_wait_seconds: v.number(),
  }),
  v.looseObject({
    error: v.literal('command_safety_rejected'),
    error_message: v.string(),
    reason: v.string(),
  }),
  v.looseObject({
    error: v.literal('spawn_failed'),
    error_message: v.string(),
  }),
  v.looseObject({
    error: v.literal('label_too_long'),
    error_message: v.string(),
    max_label_length: v.number(),
  }),
  v.looseObject({
    error: v.literal('mutually_exclusive_modes'),
    error_message: v.string(),
    conflicting_args: v.array(v.string()),
    recommended_action: v.string(),
  }),
]) satisfies v.GenericSchema<unknown, WireBashErrorResponse>;

/** Tmux tool successful response (REQ-TMUX-012). stdout / stderr are kept
 *  separate (different from bash's combined ring buffer) because tmux
 *  subcommands emit structured CLI output where the distinction matters. */
export const TmuxToolResponseSchema = v.looseObject({
  status: v.string(),
  exit_code: v.nullable(v.number()),
  duration_ms: v.number(),
  stdout: v.string(),
  stderr: v.string(),
  truncated: v.boolean(),
}) satisfies v.GenericSchema<unknown, WireTmuxToolResponse>;

/** Tmux tool error envelope (matches `error_envelope` in `src/tools/tmux.rs`). */
export const TmuxErrorResponseSchema = v.looseObject({
  error: v.string(),
  message: v.string(),
}) satisfies v.GenericSchema<unknown, WireTmuxErrorResponse>;

export type BashResponseData = v.InferOutput<typeof BashResponseSchema>;
export type BashErrorResponseData = v.InferOutput<typeof BashErrorResponseSchema>;
export type TmuxToolResponseData = v.InferOutput<typeof TmuxToolResponseSchema>;
export type TmuxErrorResponseData = v.InferOutput<typeof TmuxErrorResponseSchema>;
