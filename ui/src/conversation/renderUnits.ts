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
// how they group into turns. The MessageList feeds virtuoso exactly
// `[...historicalUnits, ...tailUnits]` with no filtering inside the render
// loop, so a historical unit's array index is also its virtuoso item index
// (the conversation-nav strip relies on this to scrollToIndex by unit).

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
  | { kind: 'streaming_agent'; key: string };

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

type MutableAgentTurnUnit = Extract<HistoricalUnit, { kind: 'agent_turn' }> & {
  toolResultsByUseId: Map<string, Message>;
};

function attachToolResult(
  toolResultsByUseId: Map<string, Message>,
  msg: Message,
): void {
  const content = msg.content as ToolResultContent | undefined;
  if (content && typeof content.tool_use_id === 'string' && content.tool_use_id) {
    toolResultsByUseId.set(content.tool_use_id, msg);
  } else {
    debug('tool result missing tool_use_id', {
      message_id: msg.message_id,
      reason: 'missing_tool_use_id',
    });
  }
}

function buildAgentTurn(
  messages: Message[],
  startIdx: number,
  isFirstInTurn: boolean,
): MutableAgentTurnUnit {
  const agent = messages[startIdx]!;
  return {
    kind: 'agent_turn',
    key: agent.message_id,
    agent,
    toolResultsByUseId: new Map<string, Message>(),
    isFirstInTurn,
  };
}

export function buildRenderUnits(inputs: BuildInputs): RenderUnits {
  const { messages, pendingMessages, convState, streamingHandle } = inputs;

  const historicalUnits: HistoricalUnit[] = [];
  let i = 0;
  let inAgentRun = false;

  let activeAgentTurn: MutableAgentTurnUnit | null = null;

  while (i < messages.length) {
    const msg = messages[i]!;
    const type = getMessageType(msg);

    if (type === 'user') {
      historicalUnits.push({ kind: 'user', key: msg.message_id, message: msg });
      activeAgentTurn = null;
      inAgentRun = false;
      i++;
    } else if (type === 'skill') {
      historicalUnits.push({ kind: 'skill', key: msg.message_id, message: msg });
      activeAgentTurn = null;
      inAgentRun = false;
      i++;
    } else if (type === 'agent') {
      const unit = buildAgentTurn(messages, i, !inAgentRun);
      historicalUnits.push(unit);
      activeAgentTurn = unit;
      inAgentRun = true;
      i++;
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
      // or tool-result ownership purposes (REQ-MLRU-003). inAgentRun and
      // activeAgentTurn are preserved.
      i++;
    } else if (type === 'tool') {
      if (activeAgentTurn) {
        attachToolResult(activeAgentTurn.toolResultsByUseId, msg);
      } else {
        debug('skipped orphan tool', {
          message_id: msg.message_id,
          reason: 'orphan_tool',
        });
      }
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
  }

  return { historicalUnits, tailUnits };
}
