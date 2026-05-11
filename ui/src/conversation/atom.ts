import * as v from 'valibot';
import type { ConversationState, Message, Conversation, ToolResultContent } from '../api';
import type { Breadcrumb } from '../types';
import {
  SseTokenDataSchema,
  SseStateChangeDataSchema,
  SseMessageDataSchema,
  SseMessageUpdatedDataSchema,
  SseAgentDoneDataSchema,
  SseConversationUpdateDataSchema,
  SseBrowserSessionStateDataSchema,
  SseSteerMessageQueuedDataSchema,
  SseErrorDataSchema,
} from '../sseSchemas';
import { parseConversationState } from '../utils';

export interface StreamingBuffer {
  text: string;
  lastSequence: number;
  startedAt: number;
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
  breadcrumbs: Breadcrumb[];
  breadcrumbSequenceIds: ReadonlySet<number>;
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
}

export interface InitPayload {
  conversation: Conversation;
  messages: Message[];
  phase: ConversationState;
  breadcrumbs: Breadcrumb[];
  breadcrumbSequenceIds: ReadonlySet<number>;
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
  | { type: 'sse_state_change'; sequenceId: number; phase: ConversationState; epoch?: number }
  | { type: 'sse_agent_done'; sequenceId: number; epoch?: number }
  | { type: 'sse_token'; sequenceId: number; delta: string; epoch?: number }
  | { type: 'sse_conversation_update'; sequenceId: number; updates: Partial<Conversation>; epoch?: number }
  | { type: 'sse_browser_session_state'; sequenceId: number; active: boolean; epoch?: number }
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
    breadcrumbs: [],
    breadcrumbSequenceIds: new Set(),
    contextWindow: { used: 0 },
    systemPrompt: null,
    lastSequenceId: 0,
    streamingBuffer: null,
    uiError: null,
    toolExecutingStartedAt: null,
    connectionEpoch: null,
    connectionState: 'connecting',
  };
}

export function breadcrumbFromPhase(
  phase: ConversationState,
  sequenceId: number
): Breadcrumb | null {
  switch (phase.type) {
    case 'tool_executing': {
      // `current_tool.name` is authoritative on both wire paths (NotifyClient
      // summary and the PersistState full-serialize) — see ToolCall's custom
      // Serialize in state_machine/state.rs.
      const toolName = phase.current_tool?.name || 'tool';
      const remaining = phase.remaining_tools?.length ?? 0;
      const label =
        remaining > 0 ? `${toolName} (+${remaining})` : toolName;
      return { type: 'tool', label, toolId: phase.current_tool?.id, sequenceId };
    }
    case 'llm_requesting': {
      const label = phase.attempt > 1 ? `LLM (retry ${phase.attempt})` : 'LLM';
      return { type: 'llm', label, sequenceId };
    }
    case 'awaiting_sub_agents': {
      const pending = phase.pending.length;
      const completed = phase.completed_results.length;
      const total = pending + completed;
      const label = `sub-agents (${completed}/${total})`;
      return { type: 'subagents', label, sequenceId };
    }
    default:
      return null;
  }
}

function deriveResultSummary(result: ToolResultContent): string {
  const MAX_LEN = 80;
  const truncate = (s: string) => (s.length > MAX_LEN ? s.slice(0, MAX_LEN - 1) + '…' : s);

  const outputText = result.content ?? result.result ?? result.error ?? '';

  if (result.is_error) {
    const firstLine = outputText.split('\n').find((l) => l.trim()) ?? 'error';
    return truncate(`error: ${firstLine.trim()}`);
  }

  const firstLine = outputText.split('\n').find((l) => l.trim()) ?? 'done';
  return truncate(firstLine.trim());
}

function applyBreadcrumb(
  breadcrumbs: Breadcrumb[],
  breadcrumbSequenceIds: ReadonlySet<number>,
  newCrumb: Breadcrumb | null,
  sequenceId: number | undefined
): { breadcrumbs: Breadcrumb[]; breadcrumbSequenceIds: ReadonlySet<number> } {
  if (!newCrumb || (sequenceId !== undefined && breadcrumbSequenceIds.has(sequenceId))) {
    return { breadcrumbs, breadcrumbSequenceIds };
  }

  let newBreadcrumbs: Breadcrumb[];
  if (newCrumb.type === 'llm') {
    // Replace existing LLM breadcrumb (handles retry label update)
    newBreadcrumbs = [...breadcrumbs.filter((b) => b.type !== 'llm'), newCrumb];
  } else if (newCrumb.type === 'subagents') {
    // Replace existing subagents breadcrumb (handles count update)
    newBreadcrumbs = [...breadcrumbs.filter((b) => b.type !== 'subagents'), newCrumb];
  } else {
    newBreadcrumbs = [...breadcrumbs, newCrumb];
  }

  const newIds =
    sequenceId !== undefined
      ? new Set([...breadcrumbSequenceIds, sequenceId])
      : breadcrumbSequenceIds;

  return { breadcrumbs: newBreadcrumbs, breadcrumbSequenceIds: newIds };
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
    case 'steer_message_queued': {
      // Validated for forward-compat parity with the live handler in
      // useConnection.ts; no reducer action needed (no-op).
      v.safeParse(SseSteerMessageQueuedDataSchema, entry);
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
    // `init`, `conversation_became_terminal`, `conversation_hard_deleted`
    // are not expected in the ring (init is per-stream; terminal/delete
    // events post-date any in-flight turn). Drop quietly.
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

      // Derive in-progress phase breadcrumb if the server breadcrumbs don't
      // already carry it. UI-only enrichment; not in the spec but preserves
      // the rendering contract for snapshots whose phase is mid-turn.
      const currentCrumb = breadcrumbFromPhase(p.phase, p.lastSequenceId);
      let snapshotBreadcrumbs = p.breadcrumbs;
      let snapshotBreadcrumbSeqIds = p.breadcrumbSequenceIds;
      if (currentCrumb) {
        const alreadyPresent = p.breadcrumbs.some(
          (b) =>
            b.type === currentCrumb.type &&
            (b.type !== 'tool' || b.toolId === currentCrumb.toolId)
        );
        if (!alreadyPresent) {
          const applied = applyBreadcrumb(
            snapshotBreadcrumbs,
            snapshotBreadcrumbSeqIds,
            currentCrumb,
            undefined
          );
          snapshotBreadcrumbs = applied.breadcrumbs;
          snapshotBreadcrumbSeqIds = applied.breadcrumbSequenceIds;
        }
      }

      const phase1Floor = isFreshConnect ? p.pendingAnchorSequenceId : atom.lastSequenceId;
      let next: ConversationAtom = {
        ...atom,
        conversationId: p.conversation.id,
        conversation: p.conversation,
        messages: mergedMessages,
        phase: p.phase,
        breadcrumbs: snapshotBreadcrumbs,
        breadcrumbSequenceIds: snapshotBreadcrumbSeqIds,
        contextWindow: p.contextWindow,
        lastSequenceId: phase1Floor,
        streamingBuffer: null,
        uiError: null,
        toolExecutingStartedAt: p.phase.type === 'tool_executing' ? Date.now() : null,
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

        // User and skill messages reset breadcrumbs to start a fresh agent turn
        const isUserMessage =
          action.message.message_type === 'user' ||
          action.message.type === 'user' ||
          action.message.message_type === 'skill';

        let breadcrumbs: Breadcrumb[] = isUserMessage
          ? [{ type: 'user', label: 'User' }]
          : a.breadcrumbs;

        // Tool result message: update matching breadcrumb with result summary
        if (!isUserMessage && action.message.message_type === 'tool') {
          const toolResult = action.message.content as ToolResultContent;
          if (toolResult.tool_use_id) {
            const matchIdx = breadcrumbs.findIndex(
              (b) => b.type === 'tool' && b.toolId === toolResult.tool_use_id
            );
            if (matchIdx >= 0) {
              const summary = deriveResultSummary(toolResult);
              breadcrumbs = [...breadcrumbs];
              breadcrumbs[matchIdx] = { ...breadcrumbs[matchIdx]!, resultSummary: summary };
            }
          }
        }

        return {
          ...a,
          messages: newMessages,
          streamingBuffer: null,
          breadcrumbs,
        };
      });
    }

    case 'sse_message_updated': {
      return applyIfNewer(atom, 'sse_message_updated', action.sequenceId, (a) => {
        const idx = a.messages.findIndex((m) => m.message_id === action.messageId);
        if (idx < 0) return a;
        // Merge `durationMs` into `display_data` so `ToolUseBlock` can read it
        // from a single place regardless of whether the message arrived via
        // reconnect (DB-persisted `display_data`) or live connection (typed wire
        // field). Both paths converge here on the client.
        const durPatch =
          action.durationMs !== undefined
            ? { display_data: { ...(a.messages[idx]!.display_data ?? {}), duration_ms: action.durationMs } }
            : {};
        const merged = {
          ...a.messages[idx]!,
          ...(action.displayData !== undefined && { display_data: action.displayData }),
          ...(action.content !== undefined && { content: action.content }),
          ...durPatch,
        };
        const newMessages = [...a.messages];
        newMessages[idx] = merged;
        return { ...a, messages: newMessages };
      });
    }

    case 'sse_state_change': {
      return applyIfNewer(atom, 'sse_state_change', action.sequenceId, (a) => {
        const newCrumb = breadcrumbFromPhase(action.phase, action.sequenceId);
        const { breadcrumbs, breadcrumbSequenceIds } = applyBreadcrumb(
          a.breadcrumbs,
          a.breadcrumbSequenceIds,
          newCrumb,
          action.sequenceId
        );
        // Track when we enter tool_executing — reset on each new tool so the
        // live elapsed counter in StateBar always reflects the current tool.
        const toolExecutingStartedAt =
          action.phase.type === 'tool_executing' ? Date.now() : null;
        return {
          ...a,
          phase: action.phase,
          breadcrumbs,
          breadcrumbSequenceIds,
          toolExecutingStartedAt,
        };
      });
    }

    case 'sse_agent_done': {
      return applyIfNewer(atom, 'sse_agent_done', action.sequenceId, (a) => ({
        ...a,
        phase: { type: 'idle' },
        streamingBuffer: null,
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
      return applyIfNewer(atom, 'sse_token', action.sequenceId, (a) => ({
        ...a,
        streamingBuffer: {
          text: (a.streamingBuffer?.text ?? '') + action.delta,
          lastSequence: action.sequenceId,
          startedAt: a.streamingBuffer?.startedAt ?? Date.now(),
        },
      }));
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

    case 'sse_error':
      // Wire-originated errors carry a sequenceId and route through the
      // standard dedup path, so a replay of the same error after reconnect
      // can't re-pop a toast the user already dismissed. Client-synthesized
      // errors (schema violations, malformed JSON) have no sequenceId and
      // apply unconditionally — they're not on the server's total order.
      if (action.sequenceId !== undefined) {
        return applyIfNewer(atom, 'sse_error', action.sequenceId, (a) => ({
          ...a,
          uiError: action.error,
        }));
      }
      return { ...atom, uiError: action.error };

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

    case 'local_phase_change':
      if (action.expectedConversationId !== atom.conversationId) return atom;
      // Optimistic client-side phase update — does NOT bump lastSequenceId.
      return { ...atom, phase: action.phase };

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
