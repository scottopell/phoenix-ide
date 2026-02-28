// Utility functions

import type { ConversationState, ToolCall, PendingSubAgent, SubAgentResult } from './api';

export function escapeHtml(str: string): string {
  if (!str) return '';
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function formatRelativeTime(isoStr: string): string {
  if (!isoStr) return '';
  const date = new Date(isoStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

export function formatShortDateTime(isoStr: string): string {
  if (!isoStr) return '';
  const date = new Date(isoStr);
  const now = new Date();
  
  const timeStr = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  
  // Same year: "Jan 5, 10:30 AM"
  // Different year: "Jan 5 '24, 10:30 AM"
  if (date.getFullYear() === now.getFullYear()) {
    const dateStr = date.toLocaleDateString([], { month: 'short', day: 'numeric' });
    return `${dateStr}, ${timeStr}`;
  }
  const dateStr = date.toLocaleDateString([], { month: 'short', day: 'numeric', year: '2-digit' });
  return `${dateStr}, ${timeStr}`;
}

export function isAgentWorking(state: ConversationState): boolean {
  switch (state.type) {
    case 'idle': case 'error': case 'terminal': case 'context_exhausted':
      return false;
    case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
    case 'awaiting_sub_agents': case 'awaiting_continuation':
    case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
      return true;
    default: state satisfies never; return false;
  }
}

export function isCancellingState(state: ConversationState): boolean {
  switch (state.type) {
    case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
      return true;
    case 'idle': case 'error': case 'terminal': case 'context_exhausted':
    case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
    case 'awaiting_sub_agents': case 'awaiting_continuation':
      return false;
    default: state satisfies never; return false;
  }
}

export function getStateDescription(state: ConversationState): string {
  switch (state.type) {
    case 'awaiting_llm':
      return 'preparing request...';
    case 'llm_requesting':
      return state.attempt > 1 ? `thinking (retry ${state.attempt})...` : 'thinking...';
    case 'tool_executing': {
      const tool = state.current_tool.input?._tool || 'tool';
      const remaining = state.remaining_tools.length;
      return remaining > 0 ? `${tool} (+${remaining} queued)` : String(tool);
    }
    case 'awaiting_sub_agents': {
      const pending = state.pending.length;
      const completed = state.completed_results.length;
      const total = pending + completed;
      if (completed > 0) return `sub-agents (${completed}/${total} done)`;
      return `waiting for ${pending} sub-agent${pending !== 1 ? 's' : ''}`;
    }
    case 'awaiting_continuation':
      return 'summarizing...';
    case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
      return 'cancelling...';
    case 'idle': case 'terminal':
      return 'ready';
    case 'error':
      return 'error';
    case 'context_exhausted':
      return 'context full';
    default: state satisfies never; return '';
  }
}

export function parseConversationState(raw: unknown): ConversationState {
  if (!raw || typeof raw !== 'object') {
    return { type: 'idle' };
  }
  const obj = raw as Record<string, unknown>;
  const type = obj['type'];
  switch (type) {
    case 'idle':
    case 'awaiting_llm':
    case 'cancelling':
    case 'terminal':
      return { type };
    case 'llm_requesting':
    case 'awaiting_continuation':
      return { type, attempt: (obj['attempt'] as number) ?? 1 };
    case 'tool_executing':
      return {
        type: 'tool_executing',
        current_tool: obj['current_tool'] as ToolCall,
        remaining_tools: (obj['remaining_tools'] as ToolCall[]) ?? [],
      };
    case 'cancelling_tool':
      return { type: 'cancelling_tool', current_tool: obj['current_tool'] as ToolCall };
    case 'awaiting_sub_agents':
      return {
        type: 'awaiting_sub_agents',
        pending: (obj['pending'] as PendingSubAgent[]) ?? [],
        completed_results: (obj['completed_results'] as SubAgentResult[]) ?? [],
      };
    case 'cancelling_sub_agents':
      return { type: 'cancelling_sub_agents', pending: (obj['pending'] as PendingSubAgent[]) ?? [] };
    case 'context_exhausted':
      return { type: 'context_exhausted', summary: (obj['summary'] as string) ?? '' };
    case 'error':
      return { type: 'error', message: (obj['message'] as string) ?? 'Unknown error' };
    default:
      console.warn(`Unknown conversation state type: ${String(type)}`);
      return { type: 'error', message: `Unknown state: ${String(type)}` };
  }
}

export function renderMarkdown(text: string): string {
  if (!text) return '';

  // Escape HTML first
  let html = escapeHtml(text);

  // Code blocks
  html = html.replace(/```([\s\S]*?)```/g, '<pre><code>$1</code></pre>');

  // Inline code
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');

  // Bold
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');

  // Italic
  html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');

  // Line breaks to paragraphs
  const paragraphs = html.split(/\n\n+/);
  html = paragraphs
    .map((p) => (p.trim() ? `<p>${p.replace(/\n/g, '<br>')}</p>` : ''))
    .join('');

  return html;
}
