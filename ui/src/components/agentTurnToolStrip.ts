// Pure derivation of a compact-mode tool pill strip from an agent turn's
// content blocks + its paired tool results.
//
// This is presentational support for the conversation density feature: it
// turns the tool_use blocks already present on an `agent_turn` render unit
// into the lightweight `{ name, toolId, isSubAgent, hasResult, isError }`
// descriptors the inline pill strip paints. It reads ONLY the turn's own
// data (content blocks + `toolResultsByUseId`), never phase/breadcrumb
// state — the source of truth for what a turn did is the turn itself.

import type { ContentBlock, Message, ToolResultContent } from '../api';

export interface ToolStripItem {
  /** Tool name as it appears on the content block (e.g. `bash`, `patch`). */
  name: string;
  /** The tool_use block id; used to key the pill and to target expansion. */
  toolId: string;
  /** spawn_agents launches sub-agents — colored distinctly in the strip. */
  isSubAgent: boolean;
  /** Whether a paired tool result has landed for this tool yet. */
  hasResult: boolean;
  /** Whether the paired result reported an error. */
  isError: boolean;
}

/**
 * Derive the compact tool strip for a single agent message. `think` blocks
 * are excluded — they are model reasoning, not actions, and already render
 * as their own self-collapsing aside. Returns one item per remaining
 * tool_use block, in document order.
 */
export function deriveToolStripItems(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
): ToolStripItem[] {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const items: ToolStripItem[] = [];
  for (const block of blocks) {
    if (block.type !== 'tool_use') continue;
    const name = block.name || 'tool';
    if (name === 'think') continue;
    const toolId = block.id || '';
    const result = toolId ? toolResultsByUseId.get(toolId) : undefined;
    const resultContent = result?.content as ToolResultContent | undefined;
    const isError = !!(resultContent?.is_error || resultContent?.error);
    items.push({
      name,
      toolId,
      isSubAgent: name === 'spawn_agents',
      hasResult: result !== undefined,
      isError,
    });
  }
  return items;
}
