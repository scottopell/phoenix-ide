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
// how they group into turns. The MessageList feeds VirtualTranscript exactly
// `[...historicalUnits, ...tailUnits]` with no filtering inside the render
// loop, so a historical unit's array index is also its VirtualTranscript item index
// (the conversation-nav strip relies on this to scrollToIndex by unit).

import type {
  ConversationState,
  Message,
  ToolResultContent,
} from '../api';
import type { PendingUserMessage } from '../hooks/useMessageQueue';

export type AwaitingSubAgentsState = Extract<
  ConversationState,
  { type: 'awaiting_sub_agents' }
>;

export interface AgentTurnUnit {
  kind: 'agent_turn';
  key: string;
  agent: Message;
  toolResultsByUseId: ReadonlyMap<string, Message>;
  isFirstInTurn: boolean;
}

export interface ToolOnlyAgentTurnGroup {
  kind: 'tool_only_agent_turn_group';
  key: string;
  members: readonly AgentTurnUnit[];
}

export type HistoricalUnit =
  | { kind: 'user'; key: string; message: Message }
  | { kind: 'skill'; key: string; message: Message }
  | AgentTurnUnit
  | ToolOnlyAgentTurnGroup
  | { kind: 'system'; key: string; message: Message }
  | { kind: 'pending_user'; key: string; message: PendingUserMessage };

export type TailUnit =
  | { kind: 'sub_agent_status'; key: string; state: AwaitingSubAgentsState }
  | { kind: 'streaming_agent'; key: string; isFirstInTurn: boolean };

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
  pendingMessages: PendingUserMessage[];
  convState: ConversationState;
  streamingHandle: StreamingHandle | null;
}

export interface HistoricalBuildInputs {
  messages: Message[];
  pendingMessages: PendingUserMessage[];
}

export interface HistoricalBuild {
  historicalUnits: HistoricalUnit[];
  /** True when the message walk ends inside an agent run (the last
   *  run-affecting message was agent/tool). Feeds the streaming tail
   *  unit's `isFirstInTurn`. Pending user messages do not affect it. */
  endsInAgentRun: boolean;
}

export function findHistoricalUnitIndexByMessageId(
  historicalUnits: readonly HistoricalUnit[],
  messageId: string,
): number {
  return historicalUnits.findIndex((unit) => {
    const agentTurns = unit.kind === 'agent_turn'
      ? [unit]
      : unit.kind === 'tool_only_agent_turn_group'
        ? unit.members
        : [];
    if (agentTurns.some((member) => (
      member.agent.message_id === messageId
      || Array.from(member.toolResultsByUseId.values()).some((message) => message.message_id === messageId)
    ))) return true;
    return 'message' in unit
      && 'message_id' in unit.message
      && unit.message.message_id === messageId;
  });
}

export interface TailBuildInputs {
  convState: ConversationState;
  streamingHandle: StreamingHandle | null;
  endsInAgentRun: boolean;
  finalizedAgentKeys?: ReadonlySet<string>;
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

type MutableAgentTurnUnit = Omit<AgentTurnUnit, 'toolResultsByUseId'> & {
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

// The transform is split at the memoization boundary MessageList needs:
// historical units depend only on (messages, pendingMessages), tail units
// only on (convState, streamingHandle) plus the walk's endsInAgentRun.
// Building both from one function keyed on all four inputs rebuilt every
// historical unit — fresh object and toolResultsByUseId Map identities —
// on every conversation state tick, defeating AgentMessage's memo() and
// re-rendering every mounted row for zero visual change.
// `buildRenderUnits` composes the halves for callers that want the whole
// transform in one step (tests, non-memoized call sites).

function isToolOnlyAgentTurn(unit: HistoricalUnit): unit is AgentTurnUnit {
  if (unit.kind !== 'agent_turn' || !Array.isArray(unit.agent.content) || unit.agent.content.length === 0) {
    return false;
  }
  return unit.agent.content.every((block) => block.type === 'tool_use' && block.name !== 'think');
}

function groupToolOnlyAgentTurns(units: HistoricalUnit[]): HistoricalUnit[] {
  const grouped: HistoricalUnit[] = [];
  let run: AgentTurnUnit[] = [];

  const flushRun = () => {
    if (run.length === 1) {
      grouped.push(run[0]!);
    } else if (run.length > 1) {
      grouped.push({
        kind: 'tool_only_agent_turn_group',
        key: run[0]!.key,
        members: run,
      });
    }
    run = [];
  };

  for (const unit of units) {
    if (isToolOnlyAgentTurn(unit)) {
      run.push(unit);
    } else {
      flushRun();
      grouped.push(unit);
    }
  }
  flushRun();
  return grouped;
}

export function agentTurnsInHistoricalUnit(unit: HistoricalUnit): readonly AgentTurnUnit[] {
  if (unit.kind === 'agent_turn') return [unit];
  if (unit.kind === 'tool_only_agent_turn_group') return unit.members;
  return [];
}

export function buildHistoricalUnits(
  inputs: HistoricalBuildInputs,
): HistoricalBuild {
  const { messages, pendingMessages } = inputs;

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

  return { historicalUnits: groupToolOnlyAgentTurns(historicalUnits), endsInAgentRun: inAgentRun };
}

export function buildTailUnits(inputs: TailBuildInputs): TailUnit[] {
  const { convState, streamingHandle, endsInAgentRun, finalizedAgentKeys } = inputs;

  const tailUnits: TailUnit[] = [];

  if (convState.type === 'awaiting_sub_agents') {
    tailUnits.push({
      kind: 'sub_agent_status',
      key: SUB_AGENT_STATUS_KEY,
      state: convState,
    });
  }

  if (streamingHandle !== null && !finalizedAgentKeys?.has(streamingHandle.key)) {
    // First in turn when no finalized agent message has appeared yet this run:
    // the live stream then renders the same `message-header` the finalized first
    // message will, so the header row doesn't pop in on finalize. A continuation
    // stream (after a tool result, endsInAgentRun still set) gets no header,
    // matching its finalized continuation message.
    tailUnits.push({
      kind: 'streaming_agent',
      key: streamingHandle.key,
      isFirstInTurn: !endsInAgentRun,
    });
  } else if (streamingHandle !== null) {
    debug('suppressed finalized streaming unit', {
      key: streamingHandle.key,
      reason: 'historical_key_owns_identity',
    });
  }

  return tailUnits;
}

export function buildRenderUnits(inputs: BuildInputs): RenderUnits {
  const { historicalUnits, endsInAgentRun } = buildHistoricalUnits(inputs);
  const tailUnits = buildTailUnits({
    convState: inputs.convState,
    streamingHandle: inputs.streamingHandle,
    endsInAgentRun,
    finalizedAgentKeys: new Set(
      historicalUnits.flatMap((unit) => agentTurnsInHistoricalUnit(unit).map((member) => member.key)),
    ),
  });
  return { historicalUnits, tailUnits };
}
