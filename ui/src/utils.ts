// Utility functions

import type { ConversationState } from './api';

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

export function formatShortDate(isoStr: string): string {
  if (!isoStr) return '';
  const date = new Date(isoStr);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();
  
  if (isToday) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
  if (isYesterday) {
    return 'yesterday';
  }
  // Show month/day for this year, full date for older
  if (date.getFullYear() === now.getFullYear()) {
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }
  return date.toLocaleDateString([], { month: 'short', day: 'numeric', year: '2-digit' });
}

export function getStateDescription(convState: string, stateData: ConversationState | null): string {
  switch (convState) {
    case 'awaiting_llm':
      return 'preparing request...';
    case 'llm_requesting': {
      const attempt = stateData?.attempt || 1;
      return attempt > 1 ? `thinking (retry ${attempt})...` : 'thinking...';
    }
    case 'tool_executing': {
      const tool = stateData?.current_tool?.input?._tool || 'tool';
      const remaining = stateData?.remaining_tools?.length ?? 0;
      return remaining > 0 ? `${tool} (+${remaining} queued)` : tool;
    }
    case 'awaiting_sub_agents': {
      const pending = stateData?.pending_ids?.length ?? 0;
      const completed = stateData?.completed_results?.length ?? 0;
      const total = pending + completed;
      if (completed > 0) {
        return `sub-agents (${completed}/${total} done)`;
      }
      return `waiting for ${pending} sub-agent${pending !== 1 ? 's' : ''}`;
    }
    case 'cancelling':
    case 'cancelling_llm':
    case 'cancelling_tool':
    case 'cancelling_sub_agents':
      return 'cancelling...';
    default:
      return convState.replace(/_/g, ' ');
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
