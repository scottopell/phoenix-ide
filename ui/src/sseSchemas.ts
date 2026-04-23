// SSE wire-format runtime validation (task 02674).
//
// Every SSE event handler in `hooks/useConnection.ts` used to cast the result
// of `JSON.parse` with `as SomeType`, which the TypeScript compiler enforces
// at compile time and *nothing* enforces at runtime. A malformed-but-parseable
// payload (missing field, wrong type, null where a string was expected) would
// reach the conversation reducer unchanged and silently corrupt state — most
// dangerously by letting a string `sequence_id` through, which breaks the
// `atom.lastSequenceId >= action.sequenceId` dedup guard via string compare.
//
// The schemas in this file are the single source of truth for what the UI
// expects to see on each SSE event channel. The corresponding TypeScript
// types are derived via `v.InferOutput<typeof ...Schema>`, so there's no
// possibility of the hand-written type drifting from the runtime validator.
//
// Strictness: object schemas are *loose* (extra top-level keys allowed). The
// Rust backend adds forward-compatible fields routinely; rejecting unknown
// keys would turn every minor server addition into a client-side crash, which
// is worse than the original silent-drift problem. What we validate is that
// required fields are present and typed correctly.

import * as v from 'valibot';
import type { Conversation, Message } from './api';

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
 *  fields (src/runtime.rs:110-134). */
const ConversationSchema = v.pipe(
  v.looseObject({ id: v.string() }),
  v.transform((obj): Conversation => obj as unknown as Conversation),
);

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
    // Mirror the Rust `MessageType` enum at src/db/schema.rs:879. The
    // picklist is strict so an unknown type surfaces as a schema violation
    // (forward-compat risk accepted for this field — new message types are
    // rare and additive). A conversation's history can include `error`
    // messages (parse-error fallback) and `continuation` messages
    // (continuation summaries), so both must be listed here — otherwise
    // init for any conversation with those in history would fail to
    // validate.
    message_type: v.picklist([
      'user',
      'agent',
      'tool',
      'system',
      'skill',
      'error',
      'continuation',
    ]),
    content: v.unknown(),
    display_data: v.optional(v.unknown()),
    usage_data: v.optional(v.unknown()),
    created_at: v.string(),
  }),
  v.transform((obj): Message => obj as unknown as Message),
);

/** Breadcrumb as it appears on the wire (snake_case) before the UI transform. */
const SseBreadcrumbSchema = v.looseObject({
  type: v.picklist(['user', 'llm', 'tool', 'subagents']),
  label: v.string(),
  tool_id: v.optional(v.string()),
  sequence_id: v.optional(v.number()),
  preview: v.optional(v.string()),
});
export type SseBreadcrumb = v.InferOutput<typeof SseBreadcrumbSchema>;

// ---------------------------------------------------------------------------
// Event schemas. One per `addEventListener` in useConnection.ts.
// ---------------------------------------------------------------------------

/** `init`: full state snapshot at connect / reconnect.
 *
 *  `conversation`, `messages`, `breadcrumbs` are the structured fields the
 *  reducer reads. `commits_behind`/`commits_ahead`/`project_name` are
 *  top-level mirrors that `transformInitData` in useConnection.ts merges
 *  back into the conversation object — they live at the top level on the
 *  wire because the Rust `SseEvent::Init` struct carries them as flat
 *  fields (src/runtime.rs:167-171). */
export const SseInitDataSchema = v.looseObject({
  conversation: ConversationSchema,
  messages: v.array(MessageSchema),
  agent_working: v.boolean(),
  last_sequence_id: v.number(),
  display_state: v.optional(v.string()),
  context_window_size: v.optional(v.number()),
  model_context_window: v.optional(v.number()),
  breadcrumbs: v.optional(v.array(SseBreadcrumbSchema)),
  commits_behind: v.optional(v.number()),
  commits_ahead: v.optional(v.number()),
  project_name: v.nullish(v.string()),
});
export type SseInitData = v.InferOutput<typeof SseInitDataSchema>;

/** `message`: a newly-created message joins the conversation. */
export const SseMessageDataSchema = v.looseObject({
  message: MessageSchema,
});
export type SseMessageData = v.InferOutput<typeof SseMessageDataSchema>;

/** `message_updated`: in-place mutation of an existing message's mutable
 *  fields. `display_data` and `content` are optional because either one can
 *  change independently — the server sends both keys (possibly `null`) every
 *  time (see `src/api/sse.rs:84-96`). */
export const SseMessageUpdatedDataSchema = v.looseObject({
  message_id: v.string(),
  display_data: v.nullish(v.unknown()),
  content: v.nullish(v.unknown()),
});
export type SseMessageUpdatedData = v.InferOutput<typeof SseMessageUpdatedDataSchema>;

/** `state_change`: conversation phase transition. The inner `state` is a
 *  discriminated union by `type` (idle / awaiting_llm / tool_executing / …).
 *  Rather than re-derive that union here, we pass the raw value to
 *  `parseConversationState` in utils.ts, which already performs its own
 *  tagged-union validation. We just assert the envelope is present. */
export const SseStateChangeDataSchema = v.looseObject({
  state: v.unknown(),
  display_state: v.optional(v.string()),
});
export type SseStateChangeData = v.InferOutput<typeof SseStateChangeDataSchema>;

/** `token`: ephemeral streaming delta during an LLM request. */
export const SseTokenDataSchema = v.looseObject({
  text: v.string(),
  request_id: v.optional(v.string()),
});
export type SseTokenData = v.InferOutput<typeof SseTokenDataSchema>;

/** `conversation_update`: partial conversation metadata update. The backend
 *  sends a strict subset of the Conversation fields (see Rust
 *  `ConversationMetadataUpdate`). We accept any object and let the reducer
 *  merge it shallowly — forward compatibility matters here more than
 *  enforcement, because new metadata fields are added frequently. */
export const SseConversationUpdateDataSchema = v.looseObject({
  conversation: v.record(v.string(), v.unknown()),
});
export type SseConversationUpdateData = v.InferOutput<typeof SseConversationUpdateDataSchema>;

/** `agent_done`: empty envelope. Still validated so that a future server
 *  change that adds fields can be discovered by a type-check or a new test
 *  rather than a silent nop. */
export const SseAgentDoneDataSchema = v.looseObject({});
export type SseAgentDoneData = v.InferOutput<typeof SseAgentDoneDataSchema>;

/** `conversation_became_terminal`: empty envelope. Wired up as a no-op today
 *  but validated so that if the server starts including teardown detail
 *  (process id, terminal session id, …) it is not silently dropped. */
export const SseConversationBecameTerminalDataSchema = v.looseObject({});
export type SseConversationBecameTerminalData = v.InferOutput<
  typeof SseConversationBecameTerminalDataSchema
>;

/** `error` (backend-application channel): distinguished from a native
 *  EventSource connection error (which arrives with no `data` at all) by
 *  the presence of a parseable JSON body. See `src/api/sse.rs:135-146` —
 *  the backend emits a flat `message` string plus a typed `error` object;
 *  the UI has historically read `message` and we keep that contract.
 *
 *  Native EventSource connection-reset errors go through a different path
 *  in useConnection.ts and do not use this schema. */
export const SseErrorDataSchema = v.looseObject({
  message: v.string(),
  error: v.optional(v.unknown()),
});
export type SseErrorData = v.InferOutput<typeof SseErrorDataSchema>;
