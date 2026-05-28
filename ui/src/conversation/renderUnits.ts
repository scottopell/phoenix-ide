// Render-unit construction for the conversation message list.
//
// Pure transform: (messages, pendingMessages, convState, streamingHandle)
// -> { historicalUnits: HistoricalUnit[]; tailUnits: TailUnit[] }.
//
// See specs/messagelist-render-units/render_units.allium for the
// behavioural spec and specs/messagelist-render-units/requirements.md
// for the REQ-MLRU-* requirements this module implements.
//
// The transform is the only place that decides which messages render and
// how they group into turns. The MessageList renders exactly
// `historicalUnits.slice(firstRenderedUnitIndex)` followed by all
// `tailUnits`, with no filtering inside the render loop.

import type {
  ConversationState,
  Message,
  ToolResultContent,
} from '../api';
import type { QueuedMessage } from '../hooks/useMessageQueue';

export type AwaitingSubAgentsState = Extract<
  ConversationState,
  { type: 'awaiting_sub_agents' }
>;

export type HistoricalUnit =
  | { kind: 'user'; key: string; message: Message }
  | { kind: 'skill'; key: string; message: Message }
  | {
      kind: 'agent_turn';
      key: string;
      agent: Message;
      toolResultsByUseId: ReadonlyMap<string, Message>;
      isFirstInTurn: boolean;
    }
  | { kind: 'system'; key: string; message: Message }
  | { kind: 'pending_user'; key: string; message: QueuedMessage };

export type TailUnit =
  | { kind: 'sub_agent_status'; key: string; state: AwaitingSubAgentsState }
  | { kind: 'streaming_agent'; key: string }
  // REQ-WPV-006: synthetic "awaiting LLM response Ns" bubble during pre-first-byte
  // llm_requesting. Lives parallel to `streaming_agent` — same screen
  // slot, different contents (elapsed counter vs streamed text). Only
  // emitted when the phase is llm_requesting AND no streaming handle
  // exists (no tokens yet for the current request). The leaf component
  // reads `phaseStateUpdatedAt` from the atom to render the live counter;
  // this carrier only conveys the key for React reconciliation.
  | { kind: 'pending_agent'; key: string };

export type RenderUnit = HistoricalUnit | TailUnit;

export interface RenderUnits {
  historicalUnits: HistoricalUnit[];
  tailUnits: TailUnit[];
}

/** A stable handle for the active streaming view. The transform reads
 *  only its `key`; the actual buffer text is subscribed to inside the
 *  StreamingMessage leaf component via the conversation atom (see
 *  REQ-MLRU-010). Passing `null` means no streaming view is active. */
export interface StreamingHandle {
  key: string;
}

export interface BuildInputs {
  messages: Message[];
  pendingMessages: QueuedMessage[];
  convState: ConversationState;
  streamingHandle: StreamingHandle | null;
}

/** Singleton key for the sub-agent status tail unit. React reconciler
 *  uses this to preserve the inner component identity across
 *  re-derivations. */
export const SUB_AGENT_STATUS_KEY = 'sub-agent-status';

/** Singleton key for the pending-assistant-bubble tail unit
 *  (REQ-WPV-006). Distinct from the streaming bubble's key (which is
 *  per-request derived from the streamingHandle) so React reconciles
 *  the two as separate components, even though they occupy the same
 *  screen slot. */
export const PENDING_AGENT_KEY = 'pending-agent';

function getMessageType(msg: Message): string {
  return msg.message_type || msg.type || '';
}

function hasNonEmptyText(msg: Message): boolean {
  const content = msg.content as { text?: string } | undefined;
  return typeof content?.text === 'string' && content.text.length > 0;
}

function debug(summary: string, fields: Record<string, unknown>): void {
  console.debug(`[renderUnits] ${summary}`, fields);
}

interface AgentTurnResult {
  unit: HistoricalUnit;
  consumed: number;
}

function buildAgentTurn(
  messages: Message[],
  startIdx: number,
  isFirstInTurn: boolean,
): AgentTurnResult {
  const agent = messages[startIdx]!;
  const toolResultsByUseId = new Map<string, Message>();
  let j = startIdx + 1;
  while (j < messages.length) {
    const next = messages[j]!;
    if (getMessageType(next) !== 'tool') break;
    const content = next.content as ToolResultContent | undefined;
    if (content && typeof content.tool_use_id === 'string' && content.tool_use_id) {
      toolResultsByUseId.set(content.tool_use_id, next);
    } else {
      debug('tool result missing tool_use_id', {
        message_id: next.message_id,
        reason: 'missing_tool_use_id',
      });
    }
    j++;
  }
  return {
    unit: {
      kind: 'agent_turn',
      key: agent.message_id,
      agent,
      toolResultsByUseId,
      isFirstInTurn,
    },
    consumed: j - startIdx,
  };
}

export function buildRenderUnits(inputs: BuildInputs): RenderUnits {
  const { messages, pendingMessages, convState, streamingHandle } = inputs;

  const historicalUnits: HistoricalUnit[] = [];
  let i = 0;
  let inAgentRun = false;

  while (i < messages.length) {
    const msg = messages[i]!;
    const type = getMessageType(msg);

    if (type === 'user') {
      historicalUnits.push({ kind: 'user', key: msg.message_id, message: msg });
      inAgentRun = false;
      i++;
    } else if (type === 'skill') {
      historicalUnits.push({ kind: 'skill', key: msg.message_id, message: msg });
      inAgentRun = false;
      i++;
    } else if (type === 'agent') {
      const { unit, consumed } = buildAgentTurn(messages, i, !inAgentRun);
      historicalUnits.push(unit);
      inAgentRun = true;
      i += consumed;
    } else if (type === 'system') {
      if (hasNonEmptyText(msg)) {
        historicalUnits.push({ kind: 'system', key: msg.message_id, message: msg });
      } else {
        debug('skipped empty system', {
          message_id: msg.message_id,
          reason: 'empty_system',
        });
      }
      // System messages do not break the agent run for header-suppression
      // purposes (REQ-MLRU-003). inAgentRun is preserved.
      i++;
    } else if (type === 'tool') {
      // Orphan tool: not consumed by a preceding agent's trailing scan.
      // Reaching this branch means either the conversation started with
      // a tool message or messages were reordered. Skip + log; never
      // emit a standalone unit (REQ-MLRU-002).
      debug('skipped orphan tool', {
        message_id: msg.message_id,
        reason: 'orphan_tool',
      });
      i++;
    } else {
      debug('skipped unknown type', {
        message_id: msg.message_id,
        message_type: type,
        reason: 'unknown_type',
      });
      i++;
    }
  }

  // Pending user messages append to historicalUnits at the tail, sharing
  // the eventual `user` unit's key (server populates message_id = localId
  // at ack). This keeps pending → sent transitions a keyed in-place
  // update on a single render unit — no cross-region promotion, no scroll
  // compensation (REQ-MLRU-001).
  for (const q of pendingMessages) {
    historicalUnits.push({ kind: 'pending_user', key: q.localId, message: q });
  }

  const tailUnits: TailUnit[] = [];

  if (convState.type === 'awaiting_sub_agents') {
    tailUnits.push({
      kind: 'sub_agent_status',
      key: SUB_AGENT_STATUS_KEY,
      state: convState,
    });
  }

  if (streamingHandle !== null) {
    tailUnits.push({
      kind: 'streaming_agent',
      key: streamingHandle.key,
    });
  } else if (
    convState.type === 'llm_requesting' ||
    convState.type === 'seeded_llm_requesting' ||
    convState.type === 'awaiting_llm'
  ) {
    // REQ-WPV-006: pre-first-byte llm_requesting with no streaming
    // buffer yet — emit a placeholder bubble so the user sees a
    // "awaiting LLM response Ns" anchor at the spot where the assistant text will
    // appear. The `streaming_agent` unit above is the post-first-byte
    // continuation of this slot; the two are mutually exclusive by
    // construction (the `else if` here).
    tailUnits.push({
      kind: 'pending_agent',
      key: PENDING_AGENT_KEY,
    });
  }

  return { historicalUnits, tailUnits };
}
