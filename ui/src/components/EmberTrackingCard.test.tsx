import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import type { Message } from '../api';
import { EMBER_TRACKING_PREFIX } from '../conversation/emberTrackingObservation';
import { UserMessage } from './MessageComponents';
import { copyToClipboard } from '../utils/clipboard';

vi.mock('../utils/clipboard', () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));
const captured = { captured_registry: { registry_revision: 2, active_count: 4, review_required_count: 1,
  items: [{ id: 'item-1', kind: 'bookmark', title: 'Prior design', title_truncated: false,
    disposition: 'tracking', revision: 1, review_required: false, latest_evidence_ordinal: 1 }],
  has_more: false, next_cursor: null, omitted_count: 0 }, requested_item_due_for_review: null,
  captured_at_us: 1791800000123456, admitted_input_sequence_id: 11 };
const rawText = EMBER_TRACKING_PREFIX + JSON.stringify(captured);
function message(text = rawText): Message {
  return { message_id: 'obs-1', sequence_id: 12, conversation_id: 'c1', message_type: 'user',
    origin: { kind: 'system_generated' }, event_presentation: null,
    content: { text, is_meta: true } as Message['content'], created_at: '2026-10-12T00:00:00Z' };
}

describe('Ember tracking message presentation', () => {
  it('keeps message anchors, timestamp, sender and original copy while initially collapsed', () => {
    const { container } = render(<UserMessage message={message()} />);
    expect(container.querySelector('#message-obs-1')).toHaveAttribute('data-sequence-id', '12');
    expect(container.querySelector('.message-time')).toHaveAttribute('title', new Date(message().created_at).toLocaleString());
    expect(container.querySelector('.message-sender')).toBeTruthy();
    expect(screen.getByText(/Ember tracking · captured snapshot · 4 active/)).toBeVisible();
    expect(container.querySelectorAll('details')).toHaveLength(2);
    expect(container.querySelector('details')).not.toHaveAttribute('open');
    expect(container.querySelector('details details')).not.toHaveAttribute('open');
    fireEvent.click(screen.getByTitle('Copy system observation'));
    expect(copyToClipboard).toHaveBeenCalledWith(rawText, expect.anything());
    fireEvent.click(screen.getByText('Captured tracking details'));
    expect(container.querySelector('details')).toHaveAttribute('open');
    expect(screen.getByText(/Historical snapshot only/)).toBeVisible();
    fireEvent.click(screen.getByText('Raw original payload'));
    const original = container.querySelector('pre');
    expect(original).toBeVisible();
    expect(original?.textContent).toBe(rawText);
  });
  it('auto-expands nested raw payload and highlights an active Find hit in the message-text fragment', () => {
    const { container } = render(<UserMessage message={message()} activeHighlight={{ owner: 'message-text', fragmentId: 'message-text', start: 1, end: 6 }} />);
    expect(container.querySelectorAll('details[open]')).toHaveLength(2);
    expect(container.querySelector('[data-fragment-id="message-text"] mark')).toHaveTextContent(rawText.slice(1, 6));
  });
  it('renders malformed content as the ordinary unaltered meta message', () => {
    const { container } = render(<UserMessage message={message(EMBER_TRACKING_PREFIX + '{bad')} />);
    expect(container.querySelector('.ember-tracking-card')).toBeNull();
    expect(container.querySelector('.message-content')).toHaveTextContent('{bad');
  });
});
