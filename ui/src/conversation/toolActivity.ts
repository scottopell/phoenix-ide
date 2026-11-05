import type { ConversationState, ToolCall } from '../api';

type ExecutingToolActivity = {
  type: 'executing';
  activeTool: ToolCall;
  queued: readonly ToolCall[];
};

type CancellingToolActivity = {
  type: 'cancelling';
  activeTool: ToolCall;
};

export type ToolActivity =
  | { type: 'none' }
  | ExecutingToolActivity
  | CancellingToolActivity;

export function getToolActivity(state: ConversationState): ToolActivity {
  switch (state.type) {
    case 'tool_executing':
      return { type: 'executing', activeTool: state.current_tool, queued: state.remaining_tools };
    case 'cancelling_tool':
      return { type: 'cancelling', activeTool: state.current_tool };
    default:
      return { type: 'none' };
  }
}

export function activeToolUseIdFromState(state: ConversationState): string | undefined {
  const activity = getToolActivity(state);
  const id = activity.type === 'none' ? undefined : activity.activeTool?.id;
  return typeof id === 'string' && id.length > 0 ? id : undefined;
}
