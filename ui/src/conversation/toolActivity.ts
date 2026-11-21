import type { ConversationState, FinalizedToolRoundResult, Message, ToolCall } from '../api';
import type { BashToolProgress } from '../generated/sse';

export type ToolActivity =
  | { type: 'none' }
  | {
      type: 'executing' | 'cancelling';
      assistantMessageId: string | undefined;
      unresolvedIds: ReadonlySet<string>;
      completed: readonly FinalizedToolRoundResult[];
    };

const NO_TOOL_ACTIVITY: ToolActivity = Object.freeze({ type: 'none' });

export function getToolActivity(state: ConversationState): ToolActivity {
  if (state.type !== 'tool_executing' && state.type !== 'cancelling_tool') return NO_TOOL_ACTIVITY;
  const ids = state.type === 'tool_executing'
    ? [state.current_tool?.id, ...state.remaining_tools.map((tool) => tool.id)]
    : [state.tool_use_id, ...state.skipped_tools.map((tool) => tool.id)];
  return {
    type: state.type === 'tool_executing' ? 'executing' : 'cancelling',
    assistantMessageId: state.assistant_message?.message_id,
    unresolvedIds: new Set(ids.filter((id): id is string => typeof id === 'string' && id.length > 0)),
    completed: (state.completed_results ?? []).filter((result): result is FinalizedToolRoundResult => 'tool_use_id' in result && 'outcome' in result),
  };
}

export function ownsToolActivity(activity: ToolActivity | undefined, message: Message): boolean {
  return activity !== undefined && activity.type !== 'none' && activity.assistantMessageId === message.message_id;
}

function transcriptOutput(outcome: FinalizedToolRoundResult['outcome']): string {
  switch (outcome.type) {
    case 'trusted_instructions':
      return 'Authenticated built-in skill instructions were delivered for this live request.';
    case 'cancelled':
      return outcome.message;
    case 'success':
    case 'error':
      return outcome.output;
  }
}

export function projectToolRound(
  message: Message,
  results: ReadonlyMap<string, Message>,
  activity: ToolActivity | undefined,
): ReadonlyMap<string, Message> {
  if (!ownsToolActivity(activity, message) || activity?.type === 'none' || activity === undefined || activity.completed.length === 0) return results;
  const projected = new Map(results);
  for (const result of activity.completed) {
    if (projected.has(result.tool_use_id)) continue;
    const outcome = result.outcome;
    projected.set(result.tool_use_id, {
      ...message,
      message_id: `${message.message_id}:live-result:${result.tool_use_id}`,
      message_type: 'tool',
      content: {
        tool_use_id: result.tool_use_id,
        content: transcriptOutput(outcome),
        is_error: outcome.type === 'error',
        ...('images' in outcome && outcome.images ? { images: outcome.images } : {}),
      },
      display_data: {
        ...('display_data' in outcome ? outcome.display_data : {}),
        ...(result.duration_ms !== undefined ? { duration_ms: result.duration_ms } : {}),
      },
      usage_data: null,
    });
  }
  return projected;
}

export function toolStartedAt(message: Message, toolId: string, activity: ToolActivity | undefined): number | undefined {
  if (!ownsToolActivity(activity, message) || activity?.type === 'none' || !activity?.unresolvedIds.has(toolId)) return undefined;
  const starts = (message.display_data as Record<string, unknown> | undefined)?.['tool_starts'] as Record<string, unknown> | undefined;
  const started = starts?.[toolId];
  return typeof started === 'number' ? started : undefined;
}

export function progressForToolRound(
  message: Message,
  progress: Readonly<Record<string, { progress: BashToolProgress }>>,
  activity: ToolActivity | undefined,
): typeof progress {
  if (!ownsToolActivity(activity, message) || activity?.type === 'none' || activity === undefined) return {};
  return Object.fromEntries(Object.entries(progress).filter(([id]) => activity.unresolvedIds.has(id) && toolStartedAt(message, id, activity) !== undefined));
}

export function unresolvedTools(state: Extract<ConversationState, { type: 'tool_executing' }>): readonly ToolCall[] {
  return [state.current_tool, ...state.remaining_tools];
}
