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

export interface ConversationAtom {
  conversationId: string | null;
  conversation: Conversation | null;
  phase: ConversationState;
  messages: Message[];
  contextWindow: { used: number };
  systemPrompt: string | null;
  lastSequenceId: number;
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
  lastSequenceId: number;
  /** ReplayRing anchor: seq of the last persisted Message at subscribe time.
   *  Every entry in `pendingEvents` has `sequence_id > pendingAnchorSequenceId`.
   *  On fresh-connect the init reducer seeds `lastSequenceId` from this value
   *  so the per-event applyIfNewer guards accept pending entries instead of
   *  dropping them as replays against `payload.lastSequenceId`. */
  pendingAnchorSequenceId: number;
  /** ReplayRing contents at subscribe time. Each entry is a wire-format
   *  `SseWireEvent` (snake_case fields, `type` discriminator). Validated
   *  per-entry at apply time via the existing per-event valibot schemas;
   *  malformed entries are skipped without crashing the whole init. */
  pendingEvents: unknown[];
  /** True iff the server-side ring overflowed since the last anchor. Treated
   *  as a soft hint: the DB snapshot is still authoritative and the safety
   *  belt advances `lastSequenceId` to the server's witnessed tip. */
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
  // `lastSequenceId`. The authoritative server-side phase change arrives
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
    lastSequenceId: 0,
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

/**
 * Single dedup guard for every wire-originated SSE action (task 02675).
 *
 * Contract: `sequenceId` is the server-assigned monotonic id for the whole
 * conversation (tokens, state_change, message, message_updated, … all share
 * one total order). If the atom has already seen an id ≥ this one, the event
 * is a replay — skip the mutation and keep `lastSequenceId` as-is. Otherwise
 * run `apply` and bump `lastSequenceId` to match.
 *
 * Why this exists — replaces four bespoke per-event guards in the old
 * reducer that had silently diverged: `sse_message` only guarded by
 * sequence_id but never by message_id (so a reconnect replay with a fresh id
 * duplicated the message); `sse_message_updated` had no guard at all;
 * `sse_token` used a separate per-connection closure counter (stalled on
 * reconnect); `sse_state_change` guarded on an id the server never
 * populated. Consolidating into one helper also makes dev-mode drops
 * observable — you see which event was rejected and why.
 */
function applyIfNewer(
  atom: ConversationAtom,
  eventType: string,
  sequenceId: number,
  apply: (a: ConversationAtom) => ConversationAtom
): ConversationAtom {
  if (atom.lastSequenceId >= sequenceId) {
    if (import.meta.env.DEV) {
      // Structured warning mirrors 02674's handleSchemaViolation: dropped
      // dispatches in dev become visible without spamming prod logs.
      console.warn('[sse] dropping replay', {
        eventType,
        incomingSeq: sequenceId,
        atomLastSeq: atom.lastSequenceId,
      });
    }
    return atom;
  }
  return { ...apply(atom), lastSequenceId: sequenceId };
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
      const isFreshConnect = atom.lastSequenceId === 0;
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

      const phase1Floor = isFreshConnect ? p.pendingAnchorSequenceId : atom.lastSequenceId;
      // streamingBuffer policy: fresh-connect always clears (atom had no
      // buffer to preserve). Reconnect preserves the existing buffer when
      // the snapshot phase is still llm_requesting AND the ring did not
      // overflow — pending tokens with seq > floor extend it via the
      // sse_token reducer, tokens at or below are dropped as replays.
      // Clearing on reconnect unconditionally would create a blank-UI
      // window the pending replay cannot rebuild because applyIfNewer
      // drops the very tokens we'd need. When pendingTruncated is true
      // we MUST clear — the missing middle of the stream is unreplayable
      // and the safety belt advances lastSequenceId past it, so any
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
        messages: mergedMessages,
        phase: p.phase,
        contextWindow: p.contextWindow,
        lastSequenceId: phase1Floor,
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
          lastSequenceId: p.lastSequenceId,
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
      // events" case (atom.lastSequenceId > payload.lastSequenceId).
      const finalLastSeq = Math.max(next.lastSequenceId, p.lastSequenceId);
      if (finalLastSeq !== next.lastSequenceId) {
        next = { ...next, lastSequenceId: finalLastSeq };
      }
      return next;
    }

    case 'sse_message': {
      // Defense-in-depth: even if applyIfNewer lets a message through, skip
      // if the message_id is already present. The task spec (§"sse_message
      // also needs id dedup") flags this as removing a load-bearing assumption
      // that the server never re-emits a known message with a fresh seq id.
      return applyIfNewer(atom, 'sse_message', action.sequenceId, (a) => {
        if (a.messages.some((m) => m.message_id === action.message.message_id)) {
          return a;
        }
        const newMessages = [...a.messages, action.message];

        return {
          ...a,
          messages: newMessages,
          streamingBuffer: null,
        };
      });
    }

    case 'sse_message_updated': {
      return applyIfNewer(atom, 'sse_message_updated', action.sequenceId, (a) => {
        const idx = a.messages.findIndex((m) => m.message_id === action.messageId);
        if (idx < 0) return a;
        const existing = a.messages[idx]!;
        const existingDisplay = (existing.display_data ?? {}) as Record<string, unknown>;
        // REQ-WPV-002: shallow-merge `displayData` rather than replace
        // wholesale. The runtime emits partial patches (e.g. just
        // `{tool_starts: {...}}` from `dispatch_tool_execution`) and
        // wholesale replacement would wipe existing keys (`bash`,
        // `retry_count`, `duration_ms`, etc.) that earlier broadcasts
        // or the persisted message set. Each emitter is responsible
        // for sending only the keys it owns.
        let mergedDisplay = existingDisplay;
        if (action.displayData !== undefined) {
          mergedDisplay = { ...existingDisplay, ...action.displayData };
          // `tool_starts` is an accumulating map keyed by tool_use_id; each
          // `dispatch_tool_execution` patch carries ONLY the newly-started
          // tool. The shallow spread above would replace the whole map, so a
          // second tool's start wipes the first tool's timestamp. Deep-merge
          // the nested map so every in-flight tool keeps its start time
          // (REQ-WPV-002).
          const prevStarts = existingDisplay['tool_starts'];
          const nextStarts = action.displayData['tool_starts'];
          const isObj = (v: unknown): v is Record<string, unknown> =>
            typeof v === 'object' && v !== null && !Array.isArray(v);
          if (isObj(prevStarts) && isObj(nextStarts)) {
            mergedDisplay = {
              ...mergedDisplay,
              tool_starts: { ...prevStarts, ...nextStarts },
            };
          }
        }
        // `durationMs` is a typed convenience field for tool-result
        // updates — merge into the same display_data so consumers
        // read from a single place regardless of source path.
        const withDuration =
          action.durationMs !== undefined
            ? { ...mergedDisplay, duration_ms: action.durationMs }
            : mergedDisplay;
        const merged = {
          ...existing,
          // Only overwrite display_data when at least one of the
          // contributing fields was present; otherwise preserve the
          // existing reference for cheap downstream equality checks.
          ...((action.displayData !== undefined || action.durationMs !== undefined) && {
            display_data: withDuration,
          }),
          ...(action.content !== undefined && { content: action.content }),
        };
        const newMessages = [...a.messages];
        newMessages[idx] = merged;
        return { ...a, messages: newMessages };
      });
    }

    case 'sse_state_change': {
      return applyIfNewer(atom, 'sse_state_change', action.sequenceId, (a) => {
        const phase =
          action.phase.type === 'error' && action.error
            ? { ...action.phase, error: action.error }
            : action.phase;
        // Track when we enter tool_executing — reset on each new tool so the
        // live elapsed counter in StateBar always reflects the current tool.
        const toolExecutingStartedAt =
          action.phase.type === 'tool_executing' ? Date.now() : null;
        return {
          ...a,
          // main (#175): `phase` is the error-enriched phase computed above.
          phase,
          // REQ-WPV-001: store the server-authoritative entry time.
          phaseStateUpdatedAt: action.stateUpdatedAt,
          // REQ-WPV-007: every phase transition resets the first-byte
          // signal. The next llm_requesting attempt starts in pre-first-
          // byte; non-llm phases (tool_executing, idle, …) don't have a
          // first-byte concept and must clear it so a subsequent
          // llm_requesting doesn't inherit a stale value.
          firstByteRequestId: null,
          toolExecutingStartedAt,
        };
      });
    }

    case 'sse_agent_done': {
      return applyIfNewer(atom, 'sse_agent_done', action.sequenceId, (a) => ({
        ...a,
        phase: { type: 'idle' },
        streamingBuffer: null,
        // REQ-WPV-007: turn boundary clears the first-byte signal. The
        // sse_state_change handler also clears it on phase transitions,
        // but agent_done can fire without a preceding state_change in
        // some terminal paths, so we clear here defensively.
        firstByteRequestId: null,
        // REQ-WPV-003 + REQ-LRV-003: clear the retry context on turn end.
        // Surfacing "(retry 2/5)" on a turn that has finished or aborted
        // is confusing (the user reads it as "still retrying").
        turnRetryContext: null,
      }));
    }

    case 'sse_llm_first_byte': {
      return applyIfNewer(atom, 'sse_llm_first_byte', action.sequenceId, (a) => ({
        ...a,
        firstByteRequestId: action.requestId,
      }));
    }

    case 'sse_llm_attempt': {
      return applyIfNewer(atom, 'sse_llm_attempt', action.sequenceId, (a) => ({
        ...a,
        turnRetryContext: {
          attempt: action.attempt,
          maxAttempts: action.maxAttempts,
          reason: action.reason,
          reasonText: reasonText(action.reason),
          backingOffMs: action.backingOffMs,
          resetsAt: action.resetsAt,
        },
      }));
    }

    case 'sse_token': {
      // Phase guard (task 24683): only accumulate a streaming buffer while
      // the conversation is actually waiting on an LLM response. Tokens that
      // arrive after the phase has left `llm_requesting` — because of a
      // scheduler race, a reconnect replay, or late drainage from a prior
      // turn — would otherwise spawn a "ghost" streaming message below the
      // already-persisted assistant message, which is the client-facing
      // half of the "message repeats itself" bug.
      //
      // `applyIfNewer` subsumes the old per-connection `tokenSequence`
      // closure (task 02675 §"sse_token reconnect stall fix"). The server now
      // allocates sequence_ids from the conversation's single counter, so
      // tokens emitted after a reconnect start at ids strictly greater than
      // anything the client has seen, and the stall goes away.
      if (atom.phase.type !== 'llm_requesting') {
        return atom;
      }
      return applyIfNewer(atom, 'sse_token', action.sequenceId, (a) => {
        // Key the streaming buffer by request_id. A retry after a mid-stream
        // failure (network / server_error / invalid_response) opens a fresh LLM
        // dispatch with a new request_id; its tokens must start a clean buffer
        // rather than concatenate onto the failed attempt's partial text. A
        // matching request_id — the common case, including a reconnect-replay
        // of the same attempt — appends as before.
        const sameRequest = a.streamingBuffer?.requestId === action.requestId;
        return {
          ...a,
          streamingBuffer: {
            text: (sameRequest ? (a.streamingBuffer?.text ?? '') : '') + action.delta,
            lastSequence: action.sequenceId,
            startedAt: sameRequest ? (a.streamingBuffer?.startedAt ?? Date.now()) : Date.now(),
            // The server's `request_id` is stable across every token of a
            // streaming session and matches the eventual `AssistantMessage.
            // message_id`. We capture it on every token (cheap; same value
            // throughout) so the render unit's key is available immediately
            // and survives a reconnect-replay that starts mid-stream.
            requestId: action.requestId,
          },
        };
      });
    }

    case 'sse_conversation_update':
      return applyIfNewer(atom, 'sse_conversation_update', action.sequenceId, (a) => {
        // Merge updated fields into the existing conversation object. If no
        // conversation exists yet (shouldn't happen — init always lands
        // first) bail out rather than synthesising one.
        if (!a.conversation) return a;
        return {
          ...a,
          conversation: { ...a.conversation, ...action.updates },
        };
      });

    case 'sse_browser_session_state':
      // REQ-BT-018: server-authoritative live-session edge. Update only if
      // this id is newer than anything we've seen. If the atom has no
      // conversation yet (init hasn't landed) we drop the event — there's
      // no struct to mutate, and init will carry the current value.
      return applyIfNewer(atom, 'sse_browser_session_state', action.sequenceId, (a) => {
        if (!a.conversation) {
          if (import.meta.env.DEV) {
            console.debug('[sse] dropping browser_session_state — no conversation', {
              sequenceId: action.sequenceId,
              active: action.active,
            });
          }
          return a;
        }
        return {
          ...a,
          conversation: { ...a.conversation, browser_session_active: action.active },
        };
      });

    case 'sse_work_scope_update':
      // REQ-WSUI-007: the push carries a COMPLETE inventory snapshot, so we
      // replace `workScope` wholesale — no delta application, no partial
      // state to reconcile. Routed through `applyIfNewer` (the same total
      // order as every other wire event) so a reconnect replay can't regress
      // to a stale snapshot.
      return applyIfNewer(atom, 'sse_work_scope_update', action.sequenceId, (a) => ({
        ...a,
        workScope: action.inventory,
      }));

    case 'sse_error':
      // Wire-originated errors carry a sequenceId and route through the
      // standard dedup path, so a replay of the same error after reconnect
      // can't re-pop a toast the user already dismissed. Client-synthesized
      // errors (schema violations, malformed JSON) have no sequenceId and
      // apply unconditionally — they're not on the server's total order.
      //
      // REQ-LRV-003: a turn-terminal `error` clears the retry context.
      // Surfacing "(retry 2/5)" on a finalised error reads as "still
      // retrying" when the turn has actually given up.
      if (action.sequenceId !== undefined) {
        return applyIfNewer(atom, 'sse_error', action.sequenceId, (a) => ({
          ...a,
          uiError: action.error,
          turnRetryContext: null,
        }));
      }
      return { ...atom, uiError: action.error, turnRetryContext: null };

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
      // Optimistic client-side phase update — does NOT bump lastSequenceId.
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
      if (atom.lastSequenceId > 0) return atom;
      return {
        ...atom,
        conversationId: action.conversationId,
        conversation: action.conversation,
        messages: action.messages,
        phase: action.phase,
        contextWindow: action.contextWindow,
      };

    case 'set_system_prompt':
      if (action.expectedConversationId !== atom.conversationId) return atom;
      return { ...atom, systemPrompt: action.systemPrompt };
  }
}
