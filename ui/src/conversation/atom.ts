import * as v from 'valibot';
import type { ConversationState, Message, Conversation } from '../api';
import type { ErrorPresentation } from '../errorPresentation';
import type { WorkScopeInventory } from '../generated/sse';
import {
  SseTokenDataSchema,
  SseStateChangeDataSchema,
  SseMessageDataSchema,
  SseMessageUpdatedDataSchema,
  SseAgentDoneDataSchema,
  SseLlmFirstByteDataSchema,
  SseLlmAttemptDataSchema,
  SseConversationUpdateDataSchema,
  SseBrowserSessionStateDataSchema,
  SseSteerMessageQueuedDataSchema,
  SseWorkScopeUpdateDataSchema,
  SseErrorDataSchema,
} from '../sseSchemas';
import { parseConversationState } from '../utils';

export interface StreamingBuffer {
  text: string;
  lastSequence: number;
  startedAt: number;
  /** The server-allocated `request_id` from the inbound `Token` SSE events.
   *  When the LLM request finalizes, the server uses this same id as the
   *  resulting `AssistantMessage.message_id`. The UI keys its streaming
   *  render unit by `request_id` and its eventual `agent_turn` render unit
   *  by `message_id` — they match, so the streaming → sent transition is
   *  an in-place keyed update on a single render unit, symmetric to
   *  REQ-MLRU-001's pending_user → user pattern. */
  requestId: string;
}

export type UIError =
  | { type: 'ParseError'; raw: string }
  | { type: 'BackendError'; message: string }
  | { type: 'ConnectionFailed'; retriesExhausted: boolean };

export interface MessageRange {
  start: number;
  end: number;
}

export interface EventGap {
  expectedNextEventSeq: number;
  firstBufferedEventSeq: number;
}

export interface PendingMessagePatch {
  eventSeq: number;
  displayData?: Record<string, unknown>;
  content?: Message['content'];
  durationMs?: number;
}

export interface PendingMessagePatchState {
  /** Highest patch event seq already materialized onto this message id, whether
   *  the patch landed live against an existing message or was replayed later
   *  from pending state after the message row arrived. The current wire has no
   *  per-message version, so event sequence is the patch freshness key. */
  lastAppliedPatchEventSeq: number;
  /** Deferred patches for a missing message target, kept in ascending event
   *  order and replayed once a later `sse_message` creates/upserts the row. */
  patches: PendingMessagePatch[];
}

export interface ConversationAtom {
  conversationId: string | null;
  conversation: Conversation | null;
  phase: ConversationState;
  messages: Message[];
  contextWindow: { used: number };
  systemPrompt: string | null;
  lastAppliedEventSeq: number;
  bufferedEventEnvelopes: Record<number, SSEAction>;
  eventGap: EventGap | null;
  contiguousMessageHighWater: number;
  messageRanges: MessageRange[];
  transcriptGeneration: number | null;
  pendingMessagePatches: Record<string, PendingMessagePatchState>;
  streamingBuffer: StreamingBuffer | null;
  uiError: UIError | null;
  /** `Date.now()` when the current `tool_executing` phase began. Reset on
   *  each new tool (a single agent turn may execute many tools sequentially).
   *  `null` when not in `tool_executing`. Used by StateBar to render a live
   *  elapsed-time counter. */
  toolExecutingStartedAt: number | null;
  /** Server clock (unix ms) at which the conversation entered its current
   *  `phase`. Sourced from `StateChange.state_updated_at` and from
   *  `Init.conversation.state_updated_at`, both RFC3339 strings on the
   *  wire that the SSE-handler converts to ms via `Date.parse(s)` once.
   *  `null` until the first Init/StateChange lands. Used by StateBar to
   *  render the working-phase elapsed counter; survives reconnect /
   *  reload because the value is server-authoritative (REQ-WPV-001). */
  phaseStateUpdatedAt: number | null;
  /** Client-clock (unix ms) of the most recent SSE event observed on this
   *  connection — any named event, including the typed `ping` keep-alive.
   *  Initialised to `Date.now()` on creation; bumped by every wire-event
   *  reducer action. Used by the heartbeat watchdog (REQ-WPV-004): when
   *  `connectionState === 'live'` AND the phase is working AND
   *  `now() - lastSseEventAt > HEARTBEAT_WATCHDOG_MS`, the StateBar
   *  surfaces a "no signal from server for Ns" degraded indicator. */
  lastSseEventAt: number;
  /** Request id of the LLM request whose first byte has been observed on
   *  this turn, or `null` if no first-byte marker has arrived since the
   *  last phase entry. Set by the `sse_llm_first_byte` reducer; cleared
   *  on every `state_change` event (the phase-entry edge — every new
   *  llm_requesting attempt starts in pre-first-byte) and on the
   *  turn-terminal `agent_done` event. Drives REQ-WPV-007's
   *  `awaiting LLM response Ns` → `streaming` transition in the StateBar and the
   *  pending bubble's spec-level `placeholder → streaming` edge
   *  (REQ-WPV-006). */
  firstByteRequestId: string | null;
  /** Per-turn retry context: most recent `LlmAttempt` for the current
   *  turn (specs/llm-retry-visibility/ REQ-LRV-001 / REQ-WPV-003). The
   *  StateBar composes "(retry K/N <reason>)" from these fields and
   *  appends as a suffix to the base reason. `null` when no retry has
   *  fired this turn. Cleared on `agent_done` (success) and on terminal
   *  `error` events; survives intra-turn phase transitions
   *  (llm_requesting → tool_executing) per REQ-WPV-003's `executing bash
   *  12s (retry 2/5)` example. */
  turnRetryContext: {
    attempt: number;
    maxAttempts: number;
    reason: 'rate_limit' | 'server_error' | 'network';
    /** Human-rendered reason ("rate limit", "server error",
     *  "network error"). Source of truth lives on the client because
     *  the wire transport is the snake_case enum. */
    reasonText: string;
    backingOffMs: number;
    /** unix ms; converted from the wire's RFC3339 string once. `null`
     *  when the wire omitted the field (non-rate-limit retries; rate
     *  limits whose 429 didn't include a reset timestamp). */
    resetsAt: number | null;
  } | null;
  /** Per-connection generation that produced the events this atom has accepted.
   *  `null` until `connection_opened` lands. Wire-originated actions tagged
   *  with a non-matching `epoch` are dropped at the reducer boundary — the
   *  cross-conversation contamination guard from task 08683.
   *
   *  Strictly monotonic within a single useConnection mount lifetime:
   *  a stale OPEN_SSE executor closure that fires after a newer one cannot
   *  regress the value. On hook remount (e.g. revisiting the same slug after
   *  navigation), the hook explicitly dispatches `connection_reset` to null
   *  this field before the new generation's `connection_opened` arrives. */
  connectionEpoch: number | null;
  /** Coarse connection lifecycle state, dispatched from `useConnection` so
   *  the UI can render a connecting/live/reconnecting/failed indicator
   *  without inferring from event timing. Always stamped with the connection
   *  epoch (rejected if stale). */
  connectionState: 'connecting' | 'live' | 'reconnecting' | 'failed';
  /** Full work-scope resource inventory (bash handles, tmux, browser) for the
   *  scope this conversation resolves to (REQ-WSUI-010). `null` until the
   *  initial `GET /api/work-scope/:scope_key/inventory` fetch or the first
   *  `work_scope_update` SSE event lands. The push carries a complete snapshot
   *  (REQ-WSUI-007), so the reducer replaces this field wholesale rather than
   *  merging — there is no partial state to reconcile. */
  workScope: WorkScopeInventory | null;
}

export interface InitPayload {
  conversation: Conversation;
  messages: Message[];
  phase: ConversationState;
  contextWindow: { used: number };
  transcriptGeneration: number;
  /** Legacy wire `last_sequence_id` converted at the UI boundary. This is an
   *  event-sequence cursor, not transcript message availability. */
  lastAppliedEventSeq: number;
  /** ReplayRing anchor: event seq of the last applied SSE event at subscribe
   *  time. Every entry in `pendingEvents` has
   *  `sequence_id > pendingAnchorSequenceId`. On fresh-connect the init
   *  reducer seeds `lastAppliedEventSeq` from this value so the pending replay
   *  helper accepts the next contiguous entry instead of treating the whole
   *  ring as already applied. */
  pendingAnchorSequenceId: number;
  /** ReplayRing contents at subscribe time. Each entry is a wire-format
   *  `SseWireEvent` (snake_case fields, `type` discriminator). Validated
   *  per-entry at apply time via the existing per-event valibot schemas;
   *  malformed entries are skipped without crashing the whole init. */
  pendingEvents: unknown[];
   /** True iff the server-side ring overflowed since the last anchor. The DB
    *  snapshot remains authoritative, but the client must not pretend it
    *  applied unseen events contiguously across the missing gap. */

  pendingTruncated: boolean;
}

// Task 02675: every wire-originated SSE action carries a `sequenceId` from
// the server's per-conversation monotonic counter. The reducer routes each
// one through a single `applyIfNewer` guard — see the comment on that helper
// for the contract.
//
// Task 08683: every wire-originated SSE action carries the `epoch` matching
// the `OPEN_SSE` generation that produced it. The reducer rejects such
// actions when `epoch !== atom.connectionEpoch`, closing the
// cross-conversation contamination window where a stale EventSource fires into
// a freshly-navigated atom.
//
// Client-originated actions take a different guard: the call site captures the
// conversationId at dispatch time and stamps it as `expectedConversationId`.
// The reducer drops the action when the atom is no longer that conversation,
// closing the contamination window where a slug-bound dispatch captured by an
// in-flight `await api.foo(...)` resolves after the user has navigated away.
export type SSEAction =
  | { type: 'sse_init'; payload: InitPayload; epoch?: number }
  | { type: 'sse_message'; message: Message; sequenceId: number; epoch?: number }
  | {
      type: 'sse_message_updated';
      sequenceId: number;
      messageId: string;
      displayData?: Record<string, unknown>;
      content?: Message['content'];
      /** Typed tool-execution duration; present only for tool-result updates. */
      durationMs?: number;
      epoch?: number;
    }
  | {
      type: 'sse_state_change';
      sequenceId: number;
      phase: ConversationState;
      /** Server clock (unix ms) for the new phase's entry time. Converted
       *  from the wire's RFC3339 `state_updated_at` once at the SSE
       *  handler boundary; the reducer stores it on the atom as a number
       *  (REQ-WPV-001). */
      stateUpdatedAt: number;
      /** Error presentation (#175): kind + auto-retry/user-resume policy,
       *  present when the new phase carries an error. */
      error?: ErrorPresentation;
      epoch?: number;
    }
  | { type: 'sse_agent_done'; sequenceId: number; epoch?: number }
  | { type: 'sse_token'; sequenceId: number; delta: string; requestId: string; epoch?: number }
  | {
      // REQ-WPV-007: first-byte marker for the LLM request identified by
      // `requestId`. The reducer stamps this on the atom so the StateBar
      // can switch from `awaiting LLM response Ns` to `streaming` (no counter); the
      // pending bubble's spec-level `placeholder → streaming` edge is
      // also gated by this signal (REQ-WPV-006). Emitted exactly once
      // per request; never on requests that error before any tokens.
      type: 'sse_llm_first_byte';
      sequenceId: number;
      requestId: string;
      epoch?: number;
    }
  | {
      // REQ-LRV-001 + REQ-WPV-003: per-turn retry context populator.
      // Emitted from the executor's Effect::ScheduleRetry handler
      // immediately before the spawned backoff sleep. The reducer
      // stamps `turnRetryContext` on the atom; the StateBar appends
      // "(retry K/N <reason>)" to the base reason.
      type: 'sse_llm_attempt';
      sequenceId: number;
      attempt: number;
      maxAttempts: number;
      reason: 'rate_limit' | 'server_error' | 'network';
      backingOffMs: number;
      /** unix ms; null when the wire omitted the field. */
      resetsAt: number | null;
      epoch?: number;
    }
  | { type: 'sse_conversation_update'; sequenceId: number; updates: Partial<Conversation>; epoch?: number }
  | { type: 'sse_browser_session_state'; sequenceId: number; active: boolean; epoch?: number }
  // REQ-WSUI-007 / REQ-WSUI-010: full-snapshot work-scope inventory push.
  // The wire payload carries the complete `WorkScopeInventory` for the
  // scope; the reducer replaces `workScope` wholesale (no delta merge).
  // The initial pull (REQ-WSUI-006) is seeded as panel-local state, not
  // through the atom, so the SSE push is the only writer of this field.
  | {
      type: 'sse_work_scope_update';
      sequenceId: number;
      inventory: WorkScopeInventory;
      epoch?: number;
    }
  // `sequenceId` is present when the error originated on the wire (server's
  // monotonic counter) and absent when it was synthesized client-side for a
  // schema / parse violation in useConnection.ts. Wire-originated errors are
  // routed through `applyIfNewer` so a replay after reconnect cannot re-pop a
  // toast the user already dismissed; client-synthesized errors are not part
  // of the server's total order and apply unconditionally.
  | { type: 'sse_error'; error: UIError; sequenceId?: number; epoch?: number }
  | { type: 'sse_sequence_consumed'; sequenceId: number; epoch?: number }
  | { type: 'clear_error' }
  // Synthesized by `useConnection` when an `OPEN_SSE` effect fires.
  // Carries the connection generation that just opened, so the atom can
  // start accepting events stamped with that epoch and reject events
  // stamped with any prior generation.
  | { type: 'connection_opened'; epoch: number }
  // Synthesized by `useConnection` on hook mount, before any `OPEN_SSE`.
  // Nulls `connectionEpoch` so the new machine's first `connection_opened`
  // is accepted even when the atom retains a higher epoch from a prior
  // visit. Carries no epoch itself — see comment above on the action class.
  | { type: 'connection_reset' }
  // Synthesized by `useConnection` on EVERY named SSE event (including
  // the typed `ping` keep-alive) before delegating to per-event reducer
  // handling. Bumps `lastSseEventAt` so the heartbeat watchdog can
  // distinguish "no signal" from "slow LLM stream still emitting".
  // No `epoch` — the watchdog is a client-side measurement of local
  // silence, not a server-stream state, so a stale executor's late
  // event-observed bump is benign (the worst it does is delay a
  // genuine watchdog trip by one tick).
  | { type: 'sse_event_observed' }
  // Coarse connection lifecycle indicator, dispatched from useConnection.
  | {
      type: 'connection_state';
      state: 'connecting' | 'live' | 'reconnecting' | 'failed';
      epoch?: number;
    }
   // Client-originated optimistic phase change. No sequence_id — not part of
   // the server's total order. Mutates `phase` only; does not touch
   // `lastAppliedEventSeq`. The authoritative server-side phase change arrives

  // later via `sse_state_change` and overrides this if it differs.
  | {
      type: 'local_phase_change';
      phase: ConversationState;
      expectedConversationId: string;
    }
  // Client-originated optimistic conversation update (e.g. model swap confirmation).
  | {
      type: 'local_conversation_update';
      updates: Partial<Conversation>;
      expectedConversationId: string;
    }
  | {
      type: 'set_initial_data';
      conversationId: string;
      conversation: Conversation;
      messages: Message[];
      phase: ConversationState;
      contextWindow: { used: number };
      transcriptGeneration: number;
      eventCursorFloor?: number;
    }
  | {
      type: 'merge_conversation_data';
      conversationId: string;
      conversation: Conversation;
      messages: Message[];
      phase: ConversationState;
      contextWindow: { used: number };
      transcriptGeneration: number;
    }
  | {
      type: 'set_system_prompt';
      systemPrompt: string | null;
      expectedConversationId: string;
    };

export function createInitialAtom(): ConversationAtom {
  return {
    conversationId: null,
    conversation: null,
    phase: { type: 'idle' },
    messages: [],
    contextWindow: { used: 0 },
    systemPrompt: null,
    lastAppliedEventSeq: 0,
    bufferedEventEnvelopes: {},
    eventGap: null,
    contiguousMessageHighWater: 0,
    messageRanges: [],
    transcriptGeneration: null,
    pendingMessagePatches: {},
    streamingBuffer: null,
    uiError: null,
    toolExecutingStartedAt: null,
    phaseStateUpdatedAt: null,
    lastSseEventAt: Date.now(),
    firstByteRequestId: null,
    turnRetryContext: null,
    connectionEpoch: null,
    connectionState: 'connecting',
    workScope: null,
  };
}

/** Human-rendered prose for an `LlmAttemptReason`. The wire transports
 *  the snake_case enum; this function is the single source of truth for
 *  the user-facing strings the StateBar appends in `(retry K/N <reason>)`.
 *  Specs: `specs/llm-retry-visibility/` REQ-LRV-002 + the consumer's
 *  `render_retry_modifier_for` helper. */
function reasonText(reason: 'rate_limit' | 'server_error' | 'network'): string {
  switch (reason) {
    case 'rate_limit':
      return 'rate limit';
    case 'server_error':
      return 'server error';
    case 'network':
      return 'network error';
  }
}

function normalizeMessageSequences(messages: Message[]): number[] {
  return [...new Set(messages.map((m) => m.sequence_id).filter((n) => Number.isFinite(n)))].sort((a, b) => a - b);
}

function buildMessageRangesFromSequences(sequences: number[]): MessageRange[] {
  if (sequences.length === 0) return [];
  const ranges: MessageRange[] = [];
  let start = sequences[0]!;
  let end = sequences[0]!;
  for (const seq of sequences.slice(1)) {
    if (seq === end + 1) {
      end = seq;
      continue;
    }
    ranges.push({ start, end });
    start = seq;
    end = seq;
  }
  ranges.push({ start, end });
  return ranges;
}

function deriveMessageSyncState(messages: Message[]): Pick<ConversationAtom, 'messageRanges' | 'contiguousMessageHighWater'> {
  const sequences = normalizeMessageSequences(messages);
  return {
    messageRanges: buildMessageRangesFromSequences(sequences),
    contiguousMessageHighWater: sequences.length === 0 ? 0 : sequences[sequences.length - 1]!,
  };
}

function withDerivedMessageSyncState(atom: ConversationAtom, messages: Message[]): ConversationAtom {
  return { ...atom, messages, ...deriveMessageSyncState(messages) };
}

function mergeMessagesByIdentity(existing: Message[], incoming: Message[]): Message[] {
  const byMessageId = new Map<string, Message>();
  const bySequenceId = new Map<number, Message>();

  const upsert = (message: Message) => {
    const priorByMessageId = byMessageId.get(message.message_id);
    if (priorByMessageId) bySequenceId.delete(priorByMessageId.sequence_id);
    const priorBySequenceId = bySequenceId.get(message.sequence_id);
    if (priorBySequenceId) byMessageId.delete(priorBySequenceId.message_id);
    byMessageId.set(message.message_id, message);
    bySequenceId.set(message.sequence_id, message);
  };

  for (const message of existing) upsert(message);
  for (const message of incoming) upsert(message);
  return [...byMessageId.values()].sort((a, b) => a.sequence_id - b.sequence_id);
}

function sortPendingPatches(patches: PendingMessagePatch[]): PendingMessagePatch[] {
  return [...patches].sort((a, b) => a.eventSeq - b.eventSeq);
}

/**
 * Centralized message-patch applicator for both live `sse_message_updated`
 * events and deferred replays from `pendingMessagePatches`.
 *
 * The current wire has no per-message version, so patch freshness is defined by
 * the SSE event sequence that carried the patch. Any patch whose `eventSeq` is
 * at or below the message id's `lastAppliedPatchEventSeq` is stale and becomes
 * an idempotent no-op. Applying a patch mutates only the message payload; it
 * must never advertise transcript availability by touching `messageRanges` or
 * `contiguousMessageHighWater`.
 */
function applyMessagePatch(
  message: Message,
  patch: PendingMessagePatch,
  lastAppliedPatchEventSeq: number,
): { message: Message; applied: boolean; lastAppliedPatchEventSeq: number } {
  if (patch.eventSeq <= lastAppliedPatchEventSeq) {
    return { message, applied: false, lastAppliedPatchEventSeq };
  }

  const existingDisplay = (message.display_data ?? {}) as Record<string, unknown>;
  let mergedDisplay = existingDisplay;
  if (patch.displayData !== undefined) {
    mergedDisplay = { ...existingDisplay, ...patch.displayData };
    const prevStarts = existingDisplay['tool_starts'];
    const nextStarts = patch.displayData['tool_starts'];
    const isObj = (v: unknown): v is Record<string, unknown> =>
      typeof v === 'object' && v !== null && !Array.isArray(v);
    if (isObj(prevStarts) && isObj(nextStarts)) {
      mergedDisplay = {
        ...mergedDisplay,
        tool_starts: { ...prevStarts, ...nextStarts },
      };
    }
  }

  const withDuration = patch.durationMs !== undefined ? { ...mergedDisplay, duration_ms: patch.durationMs } : mergedDisplay;
  const nextMessage = {
    ...message,
    ...((patch.displayData !== undefined || patch.durationMs !== undefined) && {
      display_data: withDuration,
    }),
    ...(patch.content !== undefined && { content: patch.content }),
  };

  return {
    message: nextMessage,
    applied: true,
    lastAppliedPatchEventSeq: patch.eventSeq,
  };
}

function storePendingMessagePatch(
  atom: ConversationAtom,
  messageId: string,
  patch: PendingMessagePatch,
): ConversationAtom {
  const existing = atom.pendingMessagePatches[messageId] ?? {
    lastAppliedPatchEventSeq: 0,
    patches: [],
  };
  const nextPatches = sortPendingPatches([...existing.patches, patch]);
  return {
    ...atom,
    pendingMessagePatches: {
      ...atom.pendingMessagePatches,
      [messageId]: {
        ...existing,
        patches: nextPatches,
      },
    },
  };
}

function applyPendingMessagePatchesToMessage(
  atom: ConversationAtom,
  message: Message,
): { atom: ConversationAtom; message: Message } {
  const pending = atom.pendingMessagePatches[message.message_id];
  if (!pending) return { atom, message };

  let nextMessage = message;
  let lastAppliedPatchEventSeq = pending.lastAppliedPatchEventSeq;
  for (const patch of pending.patches) {
    const applied = applyMessagePatch(nextMessage, patch, lastAppliedPatchEventSeq);
    nextMessage = applied.message;
    lastAppliedPatchEventSeq = applied.lastAppliedPatchEventSeq;
  }

  return {
    atom: {
      ...atom,
      pendingMessagePatches: {
        ...atom.pendingMessagePatches,
        [message.message_id]: {
          lastAppliedPatchEventSeq,
          patches: [],
        },
      },
    },
    message: nextMessage,
  };
}

function applyWireActionBody(atom: ConversationAtom, action: SSEAction): ConversationAtom {
  switch (action.type) {
    case 'sse_message': {
      const idx = atom.messages.findIndex((m) => m.message_id === action.message.message_id);
      if (idx >= 0) {
        return atom;
      }
      let nextAtom: ConversationAtom = { ...atom, streamingBuffer: null };
      let nextMessage = action.message;
      ({ atom: nextAtom, message: nextMessage } = applyPendingMessagePatchesToMessage(nextAtom, nextMessage));
      return withDerivedMessageSyncState(nextAtom, [...nextAtom.messages, nextMessage]);
    }
    case 'sse_message_updated': {
      const patch: PendingMessagePatch = {
        eventSeq: action.sequenceId,
        ...(action.displayData !== undefined && { displayData: action.displayData }),
        ...(action.content !== undefined && { content: action.content }),
        ...(action.durationMs !== undefined && { durationMs: action.durationMs }),
      };
      const idx = atom.messages.findIndex((m) => m.message_id === action.messageId);
      if (idx < 0) return storePendingMessagePatch(atom, action.messageId, patch);
      const existingPending = atom.pendingMessagePatches[action.messageId] ?? {
        lastAppliedPatchEventSeq: 0,
        patches: [],
      };
      const applied = applyMessagePatch(atom.messages[idx]!, patch, existingPending.lastAppliedPatchEventSeq);
      const nextPendingMessagePatches = {
        ...atom.pendingMessagePatches,
        [action.messageId]: {
          lastAppliedPatchEventSeq: applied.lastAppliedPatchEventSeq,
          patches: existingPending.patches.filter((p) => p.eventSeq > applied.lastAppliedPatchEventSeq),
        },
      };
      if (!applied.applied) {
        return { ...atom, pendingMessagePatches: nextPendingMessagePatches };
      }
      const newMessages = [...atom.messages];
      newMessages[idx] = applied.message;
      return { ...atom, messages: newMessages, pendingMessagePatches: nextPendingMessagePatches };
    }
    case 'sse_state_change': {
      const phase =
        action.phase.type === 'error' && action.error ? { ...action.phase, error: action.error } : action.phase;
      return {
        ...atom,
        phase,
        phaseStateUpdatedAt: action.stateUpdatedAt,
        firstByteRequestId: null,
        toolExecutingStartedAt: action.phase.type === 'tool_executing' ? Date.now() : null,
      };
    }
    case 'sse_agent_done':
      return {
        ...atom,
        phase: { type: 'idle' },
        streamingBuffer: null,
        firstByteRequestId: null,
        turnRetryContext: null,
      };
    case 'sse_llm_first_byte':
      return { ...atom, firstByteRequestId: action.requestId };
    case 'sse_llm_attempt':
      return {
        ...atom,
        turnRetryContext: {
          attempt: action.attempt,
          maxAttempts: action.maxAttempts,
          reason: action.reason,
          reasonText: reasonText(action.reason),
          backingOffMs: action.backingOffMs,
          resetsAt: action.resetsAt,
        },
      };
    case 'sse_sequence_consumed':
      return atom;
    case 'sse_token': {
      if (atom.phase.type !== 'llm_requesting') return atom;
      const sameRequest = atom.streamingBuffer?.requestId === action.requestId;
      return {
        ...atom,
        streamingBuffer: {
          text: (sameRequest ? (atom.streamingBuffer?.text ?? '') : '') + action.delta,
          lastSequence: action.sequenceId,
          startedAt: sameRequest ? (atom.streamingBuffer?.startedAt ?? Date.now()) : Date.now(),
          requestId: action.requestId,
        },
      };
    }
    case 'sse_conversation_update':
      if (!atom.conversation) return atom;
      return { ...atom, conversation: { ...atom.conversation, ...action.updates } };
    case 'sse_browser_session_state':
      if (!atom.conversation) return atom;
      return {
        ...atom,
        conversation: { ...atom.conversation, browser_session_active: action.active },
      };
    case 'sse_work_scope_update':
      return { ...atom, workScope: action.inventory };
    case 'sse_error':
      return { ...atom, uiError: action.error, turnRetryContext: null };
    default:
      return atom;
  }
}

function drainBufferedEventEnvelopes(atom: ConversationAtom): ConversationAtom {
  let next = atom;
  let expected = next.lastAppliedEventSeq + 1;
  let buffered = next.bufferedEventEnvelopes[expected];
  while (buffered) {
    const rest = { ...next.bufferedEventEnvelopes };
    delete rest[expected];
    next = applyWireActionBody({ ...next, bufferedEventEnvelopes: rest }, buffered);
    next = { ...next, lastAppliedEventSeq: expected };
    expected = next.lastAppliedEventSeq + 1;
    buffered = next.bufferedEventEnvelopes[expected];
  }
  const bufferedSeqs = Object.keys(next.bufferedEventEnvelopes)
    .map(Number)
    .filter((n) => Number.isFinite(n))
    .sort((a, b) => a - b);
  return {
    ...next,
    eventGap:
      bufferedSeqs.length > 0
        ? {
            expectedNextEventSeq: next.lastAppliedEventSeq + 1,
            firstBufferedEventSeq: bufferedSeqs[0]!,
          }
        : null,
  };
}

function applyContiguousWireAction(
  atom: ConversationAtom,
  action: Extract<SSEAction, { sequenceId?: number }>,
): ConversationAtom {
  const sequenceId = action.sequenceId;
  if (sequenceId === undefined) return applyWireActionBody(atom, action as SSEAction);
  const mutatesOnlyIfConversationPresent = action.type === 'sse_browser_session_state';
  if (mutatesOnlyIfConversationPresent && !atom.conversation) {
    return atom;
  }
  const expectedNext = atom.lastAppliedEventSeq + 1;
  if (sequenceId <= atom.lastAppliedEventSeq) {
    if (import.meta.env.DEV) {
      console.debug('[sse] dropping replayed event', {
        eventType: action.type,
        incomingEventSeq: sequenceId,
        lastAppliedEventSeq: atom.lastAppliedEventSeq,
      });
    }
    return atom;
  }
  if (sequenceId > expectedNext) {
    if (import.meta.env.DEV) {
      console.debug('[sse] buffering out-of-order event', {
        eventType: action.type,
        incomingEventSeq: sequenceId,
        expectedNextEventSeq: expectedNext,
      });
    }
    const bufferedEventEnvelopes = { ...atom.bufferedEventEnvelopes, [sequenceId]: action as SSEAction };
    const bufferedSeqs = Object.keys(bufferedEventEnvelopes).map(Number).sort((a, b) => a - b);
    return {
      ...atom,
      bufferedEventEnvelopes,
      eventGap: {
        expectedNextEventSeq: expectedNext,
        firstBufferedEventSeq: bufferedSeqs[0]!,
      },
    };
  }
  const applied = applyWireActionBody(atom, action as SSEAction);
  return drainBufferedEventEnvelopes({ ...applied, lastAppliedEventSeq: sequenceId, eventGap: null });
}


/**
 * Task 08683: cross-conversation contamination guard.
 *
 * Wire-originated actions carry the `epoch` of the `useConnection` `OPEN_SSE`
 * generation that produced them. When that epoch doesn't match the atom's
 * current `connectionEpoch`, the action is from a stale connection — typically
 * an EventSource that was opened for a different slug and hasn't fully
 * closed yet. Drop it.
 *
 * Returns true when the action should be dropped. Logs in dev so silent
 * drops are observable. Always returns false for actions without an
 * `epoch` field (client-originated, or the bootstrap `connection_opened`).
 */
function isStaleEpoch(atom: ConversationAtom, action: SSEAction): boolean {
  // `connection_opened` carries the new epoch as data; it must not be
  // pre-rejected by the guard. The reducer's case applies its own
  // monotonic check.
  if (action.type === 'connection_opened') return false;
  if (!('epoch' in action) || action.epoch === undefined) return false;
  // First connection on a fresh atom: connectionEpoch is null. Accepting
  // the first stamped action is what brings the atom online; rejecting
  // here would deadlock the bootstrap. The `connection_opened` event
  // dispatched alongside `OPEN_SSE` lifts `connectionEpoch` to a real
  // value before any other stamped action arrives.
  if (atom.connectionEpoch === null) return false;
  if (action.epoch === atom.connectionEpoch) return false;
  if (import.meta.env.DEV) {
    console.debug('[sse] dropping stale-epoch action', {
      actionType: action.type,
      actionEpoch: action.epoch,
      atomConnectionEpoch: atom.connectionEpoch,
    });
  }
  return true;
}

/**
 * Apply one entry from `payload.pendingEvents` to the atom by routing it
 * through `conversationReducer` as if it had arrived as a live SSE event.
 *
 * Each entry is a wire-format `SseWireEvent` (snake_case fields, `type`
 * discriminator). We validate against the same per-event valibot schema
 * the live SSE handler uses, then map to the matching `SSEAction` shape
 * and re-dispatch. The reducer's per-event `applyIfNewer` guards naturally
 * drop pending entries whose seq is at or below the current floor — the
 * exact contract that lets fresh-connect (floor = anchor) accept all
 * entries while reconnect (floor = previously-observed live tip) drops
 * replays.
 *
 * Malformed entries are skipped with a dev warn rather than crashing the
 * whole init — Phase 2 (`tasks/62001`) deliberately left `pending_events`
 * loose-typed on the wire so each entry is re-validated here.
 *
 * `init` itself is excluded from the ring by construction
 * (`sse_wire.allium`); we don't recurse into a nested `sse_init`.
 */
function applyPendingEvent(atom: ConversationAtom, entry: unknown): ConversationAtom {
  if (!entry || typeof entry !== 'object') {
    if (import.meta.env.DEV) {
      console.warn('[sse] dropping malformed pending entry (not an object)', { entry });
    }
    return atom;
  }
  const obj = entry as Record<string, unknown>;
  const type = obj['type'];
  switch (type) {
    case 'token': {
      const res = v.safeParse(SseTokenDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending token entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_token',
        sequenceId: res.output.sequence_id,
        delta: res.output.text,
        requestId: res.output.request_id,
      });
    }
    case 'state_change': {
      const res = v.safeParse(SseStateChangeDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending state_change entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_state_change',
        sequenceId: res.output.sequence_id,
        phase: parseConversationState(res.output.state),
        stateUpdatedAt: Date.parse(res.output.state_updated_at),
        ...(res.output.error ? { error: res.output.error } : {}),
      });
    }
    case 'message': {
      const res = v.safeParse(SseMessageDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending message entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_message',
        message: res.output.message,
        sequenceId: res.output.sequence_id,
      });
    }
    case 'message_updated': {
      const res = v.safeParse(SseMessageUpdatedDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending message_updated entry', { issues: res.issues });
        }
        return atom;
      }
      const data = res.output;
      return conversationReducer(atom, {
        type: 'sse_message_updated',
        sequenceId: data.sequence_id,
        messageId: data.message_id,
        ...(data.display_data != null && { displayData: data.display_data as Record<string, unknown> }),
        ...(data.content != null && { content: data.content as Message['content'] }),
        ...(data.duration_ms != null && { durationMs: data.duration_ms }),
      });
    }
    case 'agent_done': {
      const res = v.safeParse(SseAgentDoneDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending agent_done entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_agent_done',
        sequenceId: res.output.sequence_id,
      });
    }
    case 'llm_first_byte': {
      const res = v.safeParse(SseLlmFirstByteDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending llm_first_byte entry', {
            issues: res.issues,
          });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_llm_first_byte',
        sequenceId: res.output.sequence_id,
        requestId: res.output.request_id,
      });
    }
    case 'llm_attempt': {
      const res = v.safeParse(SseLlmAttemptDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending llm_attempt entry', {
            issues: res.issues,
          });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_llm_attempt',
        sequenceId: res.output.sequence_id,
        attempt: res.output.attempt,
        maxAttempts: res.output.max_attempts,
        reason: res.output.reason,
        backingOffMs: res.output.backing_off_ms,
        resetsAt: res.output.resets_at ? Date.parse(res.output.resets_at) : null,
      });
    }
    case 'conversation_update': {
      const res = v.safeParse(SseConversationUpdateDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending conversation_update entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_conversation_update',
        sequenceId: res.output.sequence_id,
        updates: res.output.conversation as Partial<Conversation>,
      });
    }
    case 'browser_session_state': {
      const res = v.safeParse(SseBrowserSessionStateDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending browser_session_state entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_browser_session_state',
        sequenceId: res.output.sequence_id,
        active: res.output.active,
      });
    }
    case 'work_scope_update': {
      const res = v.safeParse(SseWorkScopeUpdateDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending work_scope_update entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_work_scope_update',
        sequenceId: res.output.sequence_id,
        inventory: res.output.inventory,
      });
    }
    case 'steer_message_queued': {
      // Validated for forward-compat parity with the live handler in
      // useConnection.ts; no reducer action needed (no-op). Schema drift
      // still warns in DEV so a Rust-side wire change surfaces here.
      const res = v.safeParse(SseSteerMessageQueuedDataSchema, entry);
      if (!res.success && import.meta.env.DEV) {
        console.warn('[sse] dropping malformed pending steer_message_queued entry', { issues: res.issues });
      }
      return atom;
    }
    case 'error': {
      const res = v.safeParse(SseErrorDataSchema, entry);
      if (!res.success) {
        if (import.meta.env.DEV) {
          console.warn('[sse] dropping malformed pending error entry', { issues: res.issues });
        }
        return atom;
      }
      return conversationReducer(atom, {
        type: 'sse_error',
        sequenceId: res.output.sequence_id,
        error: { type: 'BackendError', message: res.output.message },
      });
    }
    // Known event types that don't belong in the ring by construction:
    // init is per-stream (never broadcast); terminal/delete events post-
    // date any in-flight turn (the broadcaster anchor resets on each
    // persisted Message, and these types arrive after the final message).
    // Drop silently — surfacing them as warnings would be noise during
    // forward-compatible server changes, not signal.
    case 'init':
    case 'conversation_became_terminal':
    case 'conversation_hard_deleted':
      return atom;
    // Truly unknown discriminator (new Rust-side variant the client
    // hasn't been updated for). Warn in DEV so the drift is observable.
    default:
      if (import.meta.env.DEV) {
        console.warn('[sse] dropping unrecognized pending entry type', { type, entry });
      }
      return atom;
  }
}

export function conversationReducer(
  atom: ConversationAtom,
  action: SSEAction
): ConversationAtom {
  if (isStaleEpoch(atom, action)) return atom;

  switch (action.type) {
    case 'sse_init': {
      const p = action.payload;

      // Phase 1 — snapshot. Apply the DB-backed view. On fresh-connect the
      // floor seeds from `pendingAnchorSequenceId` so the per-event
      // `applyIfNewer` guards in Phase 2 accept the pending entries (their
      // seqs are strictly above the anchor). On reconnect we preserve the
      // atom's existing floor so any pending entry the atom has already
      // observed live is dropped as a replay.
      //
      // Reconnect merges by message_id (replace existing with incoming +
      // append genuinely new); fresh-connect replaces entirely. See
      // `SseInitReconnectMerge` / `SseInitFreshConnect` in
      // `specs/conversation_atom/conversation_atom.allium`.
      const generationChanged = atom.transcriptGeneration !== null && atom.transcriptGeneration !== p.transcriptGeneration;
      const isFreshConnect = atom.lastAppliedEventSeq === 0 || generationChanged;
      let mergedMessages: Message[];
      if (!isFreshConnect) {
        const incomingById = new Map(p.messages.map((m) => [m.message_id, m]));
        const replaced = atom.messages.map((m) => incomingById.get(m.message_id) ?? m);
        const existingIds = new Set(atom.messages.map((m) => m.message_id));
        const appended = p.messages.filter((m) => !existingIds.has(m.message_id));
        mergedMessages = [...replaced, ...appended];
      } else {
        mergedMessages = p.messages;
      }

      const snapshotMessageAnchor = p.messages.reduce(
        (maxSeq, message) => Math.max(maxSeq, message.sequence_id),
        0,
      );
      const phase1Floor = Math.max(
        isFreshConnect ? p.pendingAnchorSequenceId : atom.lastAppliedEventSeq,
        snapshotMessageAnchor,
      );
      // streamingBuffer policy: fresh-connect always clears (atom had no
      // buffer to preserve). Reconnect preserves the existing buffer when
      // the snapshot phase is still llm_requesting AND the ring did not
      // overflow — pending tokens with seq > floor extend it via the
      // sse_token reducer, tokens at or below are dropped as replays.
      // Clearing on reconnect unconditionally would create a blank-UI
      // window the pending replay cannot rebuild because applyIfNewer
      // drops the very tokens we'd need. When pendingTruncated is true
      // we MUST clear — the missing middle of the stream is unreplayable
      // and the old event cursor could skip across the missing span, so any
      // preserved buffer would be a stale prefix that future live tokens
      // append onto (producing a gapped, corrupted message). When the
      // snapshot phase is anything else, the turn ended while
      // disconnected and we clear. See SseInitReconnectMerge in
      // specs/conversation_atom/conversation_atom.allium.
      const phase1StreamingBuffer =
        !isFreshConnect && p.phase.type === 'llm_requesting' && !p.pendingTruncated
          ? atom.streamingBuffer
          : null;
      let next: ConversationAtom = {
        ...atom,
        conversationId: p.conversation.id,
        conversation: p.conversation,
        phase: p.phase,
        contextWindow: p.contextWindow,
        lastAppliedEventSeq: phase1Floor,
        bufferedEventEnvelopes: {},
        eventGap: null,
        transcriptGeneration: p.transcriptGeneration,
        pendingMessagePatches: isFreshConnect ? {} : atom.pendingMessagePatches,
        ...deriveMessageSyncState(mergedMessages),
        messages: mergedMessages,
        streamingBuffer: phase1StreamingBuffer,
        uiError: null,
        toolExecutingStartedAt: p.phase.type === 'tool_executing' ? Date.now() : null,
        // REQ-WPV-001: seed the server-authoritative phase-entry timestamp
        // from `Init.conversation.state_updated_at` (RFC3339 on the wire,
        // converted to ms here). Falls back to `null` if the server
        // omitted it (old payload during rollout, or a brand-new
        // conversation that hasn't yet had a state transition).
        phaseStateUpdatedAt: p.conversation.state_updated_at
          ? Date.parse(p.conversation.state_updated_at)
          : null,
        // (Watchdog clock `lastSseEventAt` is bumped by the unconditional
        // `sse_event_observed` dispatch in useConnection's on() wrapper, the
        // single source for it — see the `sse_event_observed` case.)
        // REQ-WPV-007: an Init snapshot lands the authoritative phase;
        // any first-byte signal from before this Init is by definition
        // pre-reset state and must not bleed forward. If the replayed
        // pending events contain a first-byte for the current request,
        // the SseLlmFirstByteDataSchema path below re-stamps it.
        firstByteRequestId: null,
        // REQ-WPV-003 / REQ-LRV: same reasoning for the retry suffix. Clear
        // any retry context carried over from before this Init — otherwise a
        // reconnect-init after a finished retried turn (whose `agent_done`
        // replay was missed, e.g. truncated ring) would preserve a stale
        // `(retry N)` and surface it on the NEXT turn. If the turn is
        // genuinely mid-backoff, the replayed `llm_attempt` in Phase 2
        // re-stamps it.
        turnRetryContext: null,
      };

      // Phase 2 — fold pending events through the reducer. Each entry is a
      // wire-format SseWireEvent; per-entry validation gates against the
      // existing valibot schemas before dispatch. Malformed entries are
      // skipped with a dev warn rather than crashing the whole init.
      if (p.pendingTruncated && import.meta.env.DEV) {
        console.debug('[sse] init pendingTruncated=true — server ring overflowed; DB-only render', {
          pendingAnchorSequenceId: p.pendingAnchorSequenceId,
          lastAppliedEventSeq: p.lastAppliedEventSeq,
          pendingCount: p.pendingEvents.length,
        });
      }
      for (const entry of p.pendingEvents) {
        next = applyPendingEvent(next, entry);
      }

      // Phase 3 — safety belt. Cover the cases where pendingEvents was
      // truncated/empty but the server's witnessed tip is ahead of the
      // anchor: advance the floor so future live events with lower seqs are
      // correctly dropped. Also covers the original "stale-init lags live
      // events" case where the reconnect snapshot lags the live cursor.
      if (p.pendingTruncated) {
        next = {
          ...next,
          lastAppliedEventSeq: p.lastAppliedEventSeq,
          bufferedEventEnvelopes: {},
          eventGap: null,
          pendingMessagePatches: {},
        };
      }
      return next;
    }

    case 'sse_message': {
      const knownMessage = atom.messages.some((m) => m.message_id === action.message.message_id);
      const applied = applyWireActionBody(atom, action);
      if (action.sequenceId === atom.lastAppliedEventSeq + 1) {
        return drainBufferedEventEnvelopes({ ...applied, lastAppliedEventSeq: action.sequenceId, eventGap: null });
      }
      if (knownMessage) {
        return applied;
      }
      return applied;
    }

    case 'sse_message_updated':
    case 'sse_state_change':
    case 'sse_agent_done':
    case 'sse_llm_first_byte':
    case 'sse_llm_attempt':
    case 'sse_token':
    case 'sse_conversation_update':
    case 'sse_browser_session_state':
    case 'sse_work_scope_update':
      return applyContiguousWireAction(atom, action);

    case 'sse_sequence_consumed':
      return applyContiguousWireAction(atom, action);

    case 'sse_error':
      return action.sequenceId !== undefined
        ? applyContiguousWireAction(atom, action)
        : { ...atom, uiError: action.error, turnRetryContext: null };

    case 'clear_error':
      return { ...atom, uiError: null };

    case 'connection_opened': {
      // Strictly monotonic. A stale OPEN_SSE executor closure firing after a
      // newer one already advanced the atom must not regress the epoch — that
      // would re-accept events the new generation has already superseded. The
      // hook handles legitimate remount via an explicit `connection_reset`,
      // so by the time this case runs an out-of-order epoch is always stale.
      if (atom.connectionEpoch !== null && action.epoch <= atom.connectionEpoch) {
        if (import.meta.env.DEV) {
          console.debug('[sse] dropping stale connection_opened', {
            actionEpoch: action.epoch,
            atomConnectionEpoch: atom.connectionEpoch,
          });
        }
        return atom;
      }
      return { ...atom, connectionEpoch: action.epoch };
    }

    case 'connection_reset':
      // Hook remount: drop the retained epoch so the new generation's
      // `connection_opened` (which may be a lower number than what we last
      // saw) can lift the atom forward. Reset the visible state too so the
      // UI shows `connecting` until the new stream actually opens.
      return { ...atom, connectionEpoch: null, connectionState: 'connecting' };

    case 'connection_state':
      return { ...atom, connectionState: action.state };

    case 'sse_event_observed':
      // REQ-WPV-004: cheap, no-payload action fired from useConnection's
      // event wrapper on every named SSE event. Bumps the watchdog
      // clock so a working phase with active token flow doesn't false-
      // positive into "no signal from server."
      //
      // This is the SINGLE source for `lastSseEventAt`. Per-event reducers
      // deliberately do NOT bump it: this dispatch is unconditional (fires
      // before the event's own action and is not gated by `applyIfNewer`),
      // so even a stale/duplicate event or a payload-less keep-alive `ping`
      // correctly resets the heartbeat — "any observed traffic = alive."
      return { ...atom, lastSseEventAt: Date.now() };

    case 'local_phase_change':
      if (action.expectedConversationId !== atom.conversationId) return atom;
      // Optimistic client-side phase update — does NOT bump lastAppliedEventSeq.
      // Clear `phaseStateUpdatedAt` so the StateBar / pending bubble do not
      // render an elapsed counter using the *previous* phase's server
      // timestamp. The next server `state_change` will install the
      // authoritative entry time for the new phase.
      return { ...atom, phase: action.phase, phaseStateUpdatedAt: null };

    case 'local_conversation_update':
      if (action.expectedConversationId !== atom.conversationId) return atom;
      if (!atom.conversation) return atom;
      return { ...atom, conversation: { ...atom.conversation, ...action.updates } };

    case 'set_initial_data':
      // Don't overwrite if SSE has already provided authoritative data
      if (atom.lastAppliedEventSeq > 0) return atom;
      return {
        ...atom,
        conversationId: action.conversationId,
        conversation: action.conversation,
        messages: action.messages,
        phase: action.phase,
        contextWindow: action.contextWindow,
        transcriptGeneration: action.transcriptGeneration,
        lastAppliedEventSeq: Math.max(atom.lastAppliedEventSeq, action.eventCursorFloor ?? 0),
      };

    case 'merge_conversation_data':
      if (atom.conversationId !== null && atom.conversationId !== action.conversationId) return atom;
      const messages = mergeMessagesByIdentity(atom.messages, action.messages);
      return {
        ...atom,
        conversationId: action.conversationId,
        conversation: action.conversation,
        messages,
        phase: action.phase,
        contextWindow: action.contextWindow,
        transcriptGeneration: action.transcriptGeneration,
        ...deriveMessageSyncState(messages),
      };

    case 'set_system_prompt':
      if (action.expectedConversationId !== atom.conversationId) return atom;
      return { ...atom, systemPrompt: action.systemPrompt };
  }
}
