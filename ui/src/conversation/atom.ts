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

export type SSEAction =
  | { type: 'sse_init'; payload: InitPayload }
  | { type: 'sse_message'; message: Message; sequenceId: number }
  | { type: 'sse_state_change'; phase: ConversationState; sequenceId?: number }
  | { type: 'sse_agent_done'; sequenceId?: number }
  | { type: 'sse_token'; delta: string; sequence: number }
  | { type: 'sse_conversation_update'; updates: Partial<Conversation> }
  | { type: 'sse_error'; error: UIError }
  | { type: 'clear_error' }
  | { type: 'connection_state'; state: ConversationAtom['connectionState'] }
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

export function conversationReducer(
  atom: ConversationAtom,
  action: SSEAction
): ConversationAtom {
  switch (action.type) {
    case 'sse_init': {
      const p = action.payload;

      // When reconnecting with ?after=N, the server returns only delta messages
      // (sequence_id > N). Merge with existing to preserve full history. On
      // fresh connect (lastSequenceId=0), replace.
      //
      // Defensive dedup (task 24683): filter incoming messages by
      // sequence_id AND message_id before concatenating. The server contract
      // is "deltas only", but the client must not rely on that — any
      // accidental overlap (backend off-by-one, retry, or regression) would
      // otherwise surface as visibly duplicated messages in the chat, fixable
      // only by a full reload. `sse_message` already dedups; `sse_init` must
      // match that discipline.
      let mergedMessages: Message[];
      if (atom.lastSequenceId > 0) {
        const existingIds = new Set(atom.messages.map((m) => m.message_id));
        const delta = p.messages.filter(
          (m) => m.sequence_id > atom.lastSequenceId && !existingIds.has(m.message_id)
        );
        mergedMessages = [...atom.messages, ...delta];
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

      return {
        ...atom,
        conversationId: p.conversation.id,
        conversation: p.conversation,
        messages: mergedMessages,
        phase: p.phase,
        breadcrumbs: finalBreadcrumbs,
        breadcrumbSequenceIds: finalBreadcrumbSeqIds,
        contextWindow: p.contextWindow,
        lastSequenceId: p.lastSequenceId,
        streamingBuffer: null,
        uiError: null,
      };
    }

    case 'sse_message': {
      if (atom.lastSequenceId >= action.sequenceId) return atom;

      // Support update-in-place for messages with same message_id (e.g., display_data updates)
      const existingIdx = atom.messages.findIndex(
        (m) => m.message_id === action.message.message_id
      );
      let newMessages: Message[];
      if (existingIdx >= 0) {
        newMessages = [...atom.messages];
        newMessages[existingIdx] = action.message;
      } else {
        newMessages = [...atom.messages, action.message];
      }

      // User and skill messages reset breadcrumbs to start a fresh agent turn
      const isUserMessage =
        action.message.message_type === 'user' || action.message.type === 'user'
        || action.message.message_type === 'skill';

      let breadcrumbs: Breadcrumb[] = isUserMessage
        ? [{ type: 'user', label: 'User' }]
        : atom.breadcrumbs;

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
        ...atom,
        messages: newMessages,
        lastSequenceId: action.sequenceId,
        streamingBuffer: null,
        breadcrumbs,
      };
    }

    case 'sse_state_change': {
      if (
        action.sequenceId !== undefined &&
        atom.lastSequenceId >= action.sequenceId
      ) {
        return atom;
      }

      const newCrumb = breadcrumbFromPhase(
        action.phase,
        action.sequenceId ?? atom.lastSequenceId
      );
      const { breadcrumbs, breadcrumbSequenceIds } = applyBreadcrumb(
        atom.breadcrumbs,
        atom.breadcrumbSequenceIds,
        newCrumb,
        action.sequenceId
      );

      return {
        ...atom,
        phase: action.phase,
        breadcrumbs,
        breadcrumbSequenceIds,
        ...(action.sequenceId !== undefined
          ? { lastSequenceId: action.sequenceId }
          : {}),
      };
    }

    case 'sse_agent_done': {
      if (
        action.sequenceId !== undefined &&
        atom.lastSequenceId >= action.sequenceId
      ) {
        return atom;
      }
      return {
        ...atom,
        phase: { type: 'idle' },
        streamingBuffer: null,
        ...(action.sequenceId !== undefined
          ? { lastSequenceId: action.sequenceId }
          : {}),
      };
    }

    case 'sse_token': {
      // Phase guard (task 24683): only accumulate a streaming buffer while
      // the conversation is actually waiting on an LLM response. Tokens that
      // arrive after the phase has left `llm_requesting` — because of a
      // scheduler race, a reconnect replay, or late drainage from a prior
      // turn — would otherwise spawn a "ghost" streaming message below the
      // already-persisted assistant message, which is the client-facing
      // half of the "message repeats itself" bug.
      if (atom.phase.type !== 'llm_requesting') {
        return atom;
      }
      if (
        atom.streamingBuffer &&
        atom.streamingBuffer.lastSequence >= action.sequence
      ) {
        return atom;
      }
      return {
        ...atom,
        streamingBuffer: {
          text: (atom.streamingBuffer?.text ?? '') + action.delta,
          lastSequence: action.sequence,
          startedAt: atom.streamingBuffer?.startedAt ?? Date.now(),
        },
      };
    }

    case 'sse_error':
      return { ...atom, uiError: action.error };

    case 'clear_error':
      return { ...atom, uiError: null };

    case 'connection_state':
      return { ...atom, connectionState: action.state };

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

    case 'sse_conversation_update':
      // Merge updated fields into the existing conversation object
      if (!atom.conversation) return atom;
      return {
        ...atom,
        conversation: { ...atom.conversation, ...action.updates },
      };

    case 'set_system_prompt':
      return { ...atom, systemPrompt: action.systemPrompt };
  }
}
