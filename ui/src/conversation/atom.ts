import type { ConversationState, Message, Conversation, ToolResultContent } from '../api';
import type { Breadcrumb } from '../types';

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
  connectionState: 'connecting' | 'live' | 'reconnecting' | 'failed';
  streamingBuffer: StreamingBuffer | null;
  uiError: UIError | null;
}

export interface InitPayload {
  conversation: Conversation;
  messages: Message[];
  phase: ConversationState;
  breadcrumbs: Breadcrumb[];
  breadcrumbSequenceIds: ReadonlySet<number>;
  contextWindow: { used: number };
  lastSequenceId: number;
}

// Task 02675: every wire-originated SSE action carries a `sequenceId` from
// the server's per-conversation monotonic counter. The reducer routes each
// one through a single `applyIfNewer` guard — see the comment on that helper
// for the contract.
//
// Client-originated actions (`local_phase_change`, `local_conversation_update`,
// `set_initial_data`, `connection_state`, `set_system_prompt`, `sse_error`,
// `clear_error`) do not go through `applyIfNewer` and do not mutate
// `lastSequenceId`. They exist so UI code can optimistically reflect a
// user action (e.g. "I pressed send; show 'awaiting_llm'") without
// colliding with the server's total order — the real server-side phase
// change arrives afterwards via `sse_state_change` and takes precedence
// when its sequence_id lands.
export type SSEAction =
  | { type: 'sse_init'; payload: InitPayload }
  | { type: 'sse_message'; message: Message; sequenceId: number }
  | {
      type: 'sse_message_updated';
      sequenceId: number;
      messageId: string;
      displayData?: Record<string, unknown>;
      content?: Message['content'];
    }
  | { type: 'sse_state_change'; sequenceId: number; phase: ConversationState }
  | { type: 'sse_agent_done'; sequenceId: number }
  | { type: 'sse_token'; sequenceId: number; delta: string }
  | { type: 'sse_conversation_update'; sequenceId: number; updates: Partial<Conversation> }
  | { type: 'sse_error'; error: UIError }
  | { type: 'clear_error' }
  | { type: 'connection_state'; state: ConversationAtom['connectionState'] }
  // Client-originated optimistic phase change. No sequence_id — not part of
  // the server's total order. Mutates `phase` only; does not touch
  // `lastSequenceId`. The authoritative server-side phase change arrives
  // later via `sse_state_change` and overrides this if it differs.
  | { type: 'local_phase_change'; phase: ConversationState }
  // Client-originated optimistic conversation update (e.g. model swap confirmation).
  | { type: 'local_conversation_update'; updates: Partial<Conversation> }
  | {
      type: 'set_initial_data';
      conversationId: string;
      conversation: Conversation;
      messages: Message[];
      phase: ConversationState;
      contextWindow: { used: number };
    }
  | { type: 'set_system_prompt'; systemPrompt: string | null };

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
    connectionState: 'connecting',
    streamingBuffer: null,
    uiError: null,
  };
}

export function breadcrumbFromPhase(
  phase: ConversationState,
  sequenceId: number
): Breadcrumb | null {
  switch (phase.type) {
    case 'tool_executing': {
      // current_tool.name comes from the NotifyClient summary path;
      // current_tool.input._tool comes from the PersistState full-serialize path.
      const toolName =
        phase.current_tool?.input?._tool ||
        (phase.current_tool as { name?: string } | undefined)?.name ||
        'tool';
      const remaining = phase.remaining_tools?.length ?? 0;
      const label =
        remaining > 0 ? `${String(toolName)} (+${remaining})` : String(toolName);
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
      // eslint-disable-next-line no-console
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

export function conversationReducer(
  atom: ConversationAtom,
  action: SSEAction
): ConversationAtom {
  switch (action.type) {
    case 'sse_init': {
      const p = action.payload;

      // On fresh connect (lastSequenceId=0): replace entirely.
      // On reconnect (lastSequenceId>0): the server always returns the full
      // message list so we get a current snapshot of any mutable state. Merge
      // by replacing existing messages with the incoming version (handles
      // display_data/content mutations that occurred while disconnected) and
      // appending genuinely new messages.
      let mergedMessages: Message[];
      if (atom.lastSequenceId > 0) {
        const incomingById = new Map(p.messages.map((m) => [m.message_id, m]));
        // Replace existing messages with incoming version if present (captures mutations).
        const replaced = atom.messages.map((m) => incomingById.get(m.message_id) ?? m);
        const existingIds = new Set(atom.messages.map((m) => m.message_id));
        const appended = p.messages.filter((m) => !existingIds.has(m.message_id));
        mergedMessages = [...replaced, ...appended];
      } else {
        mergedMessages = p.messages;
      }

      // Apply in-progress phase breadcrumb if the server breadcrumbs don't include it
      const currentCrumb = breadcrumbFromPhase(p.phase, p.lastSequenceId);
      let finalBreadcrumbs = p.breadcrumbs;
      let finalBreadcrumbSeqIds = p.breadcrumbSequenceIds;

      if (currentCrumb) {
        const alreadyPresent = p.breadcrumbs.some(
          (b) =>
            b.type === currentCrumb.type &&
            (b.type !== 'tool' || b.toolId === currentCrumb.toolId)
        );
        if (!alreadyPresent) {
          const applied = applyBreadcrumb(
            finalBreadcrumbs,
            finalBreadcrumbSeqIds,
            currentCrumb,
            undefined
          );
          finalBreadcrumbs = applied.breadcrumbs;
          finalBreadcrumbSeqIds = applied.breadcrumbSequenceIds;
        }
      }

      // Init uses max() rather than replace for lastSequenceId. Rationale:
      // task 02675 §2 — "Server-side sequence jumps strand messages". If
      // init arrives with lastSequenceId=100 but we've already seen id 105
      // from live events that raced ahead, we must not regress to 100 (that
      // would re-accept the 101–105 events on re-delivery and corrupt
      // state). `max()` keeps the floor monotonically non-decreasing.
      const newLastSeq = Math.max(atom.lastSequenceId, p.lastSequenceId);

      return {
        ...atom,
        conversationId: p.conversation.id,
        conversation: p.conversation,
        messages: mergedMessages,
        phase: p.phase,
        breadcrumbs: finalBreadcrumbs,
        breadcrumbSequenceIds: finalBreadcrumbSeqIds,
        contextWindow: p.contextWindow,
        lastSequenceId: newLastSeq,
        streamingBuffer: null,
        uiError: null,
      };
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
        const merged = {
          ...a.messages[idx]!,
          ...(action.displayData !== undefined && { display_data: action.displayData }),
          ...(action.content !== undefined && { content: action.content }),
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
        return {
          ...a,
          phase: action.phase,
          breadcrumbs,
          breadcrumbSequenceIds,
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

    case 'sse_error':
      return { ...atom, uiError: action.error };

    case 'clear_error':
      return { ...atom, uiError: null };

    case 'connection_state':
      return { ...atom, connectionState: action.state };

    case 'local_phase_change':
      // Optimistic client-side phase update — does NOT bump lastSequenceId.
      return { ...atom, phase: action.phase };

    case 'local_conversation_update':
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
      return { ...atom, systemPrompt: action.systemPrompt };
  }
}
