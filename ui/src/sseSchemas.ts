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
import type { Conversation, Message } from './api';
// Generated wire types — aliased so we can reuse the short `Sse*Data`
// names for the transform-output types consumers actually want.
import type {
  SseInitData as WireInitData,
  SseMessageData as WireMessageData,
  SseMessageUpdatedData as WireMessageUpdatedData,
  SseStateChangeData as WireStateChangeData,
  SseTokenData as WireTokenData,
  SseAgentDoneData as WireAgentDoneData,
  SseConversationBecameTerminalData as WireConversationBecameTerminalData,
  SseConversationUpdateData as WireConversationUpdateData,
  SseErrorData as WireErrorData,
  SseBreadcrumb as GeneratedSseBreadcrumb,
  ChainQaTokenData as WireChainQaTokenData,
  ChainQaCompletedData as WireChainQaCompletedData,
  ChainQaFailedData as WireChainQaFailedData,
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
 *  reducer reads. `commits_behind`/`commits_ahead`/`project_name` are
 *  top-level mirrors that `transformInitData` in useConnection.ts merges
 *  back into the conversation object — they live at the top level on the
 *  wire because the Rust `SseEvent::Init` struct carries them as flat
 *  fields.
 *
 *  `sequence_id` and `last_sequence_id` are the same number by construction
 *  (the snapshot's own place in the total order equals the highest-ever-
 *  emitted id at subscribe time). Both are required.
 *
 *  `display_state` is `string` (not optional) in the Rust wire type — task
 *  02677 tightened this field from the previously-optional schema shape
 *  after the generated type surfaced the actual wire contract. */
export const SseInitDataSchema = v.looseObject({
  sequence_id: v.number(),
  conversation: ConversationSchema,
  messages: v.array(MessageSchema),
  agent_working: v.boolean(),
  last_sequence_id: v.number(),
  display_state: v.string(),
  context_window_size: v.number(),
  model_context_window: v.number(),
  breadcrumbs: v.array(SseBreadcrumbSchema),
  commits_behind: v.number(),
  commits_ahead: v.number(),
  project_name: v.nullable(v.string()),
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

/** `state_change`: conversation phase transition. The inner `state` is a
 *  discriminated union by `type` (idle / awaiting_llm / tool_executing / …).
 *  Rather than re-derive that union here, we pass the raw value to
 *  `parseConversationState` in utils.ts, which already performs its own
 *  tagged-union validation. We just assert the envelope is present. */
export const SseStateChangeDataSchema = v.looseObject({
  sequence_id: v.number(),
  state: v.unknown(),
  display_state: v.string(),
}) satisfies v.GenericSchema<unknown, WireStateChangeData>;

/** `token`: ephemeral streaming delta during an LLM request. */
export const SseTokenDataSchema = v.looseObject({
  sequence_id: v.number(),
  text: v.string(),
  request_id: v.string(),
}) satisfies v.GenericSchema<unknown, WireTokenData>;

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
