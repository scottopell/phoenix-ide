import type { Message } from '../api';
import type { ReactionSource } from '../conversation/InlineReactionStore';

function parentElement(node: Node | null): Element | null {
  return node instanceof Element ? node : node?.parentElement ?? null;
}

export function readReactionSelection(selection: Selection | null, messages: Message[]): { source: ReactionSource; range: Range } | null {
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1 || !selection.toString().trim()) return null;
  const range = selection.getRangeAt(0);
  const start = parentElement(range.startContainer);
  const end = parentElement(range.endContainer);
  if (start?.closest('input, textarea, [contenteditable]') || end?.closest('input, textarea, [contenteditable]')) return null;
  const owner = start?.closest<HTMLElement>('[data-inline-reaction-message]');
  if (!owner || owner !== end?.closest('[data-inline-reaction-message]') || !owner.closest('#messages')) return null;
  if (!start?.closest('.agent-text-block') || !end?.closest('.agent-text-block')) return null;
  if (range.cloneContents().querySelector('button, input, textarea, .tool-block, .compact-tool-strip')) return null;
  const occurrence = owner.dataset['messageOccurrence'];
  const message = messages.find((candidate) => occurrence
    ? (candidate.display_data as { productOccurrenceToken?: string } | null)?.productOccurrenceToken === occurrence
    : candidate.message_id === owner.dataset['inlineReactionMessage']);
  const data = message?.display_data as { productOccurrenceToken?: string; productHistoricalHandoff?: unknown } | null;
  if (!message || message.message_type !== 'agent' || data?.productHistoricalHandoff) return null;
  const prefix = document.createRange();
  prefix.selectNodeContents(owner);
  prefix.setEnd(range.startContainer, range.startOffset);
  const startOffset = prefix.toString().length;
  return {
    source: {
      messageId: message.message_id,
      sequenceId: message.sequence_id,
      occurrenceToken: data?.productOccurrenceToken,
      quote: selection.toString(),
      textOffsets: { start: startOffset, end: startOffset + range.toString().length },
    },
    range: range.cloneRange(),
  };
}
