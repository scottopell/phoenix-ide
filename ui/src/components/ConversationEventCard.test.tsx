import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import * as v from 'valibot';
import type { EventPresentation } from '../generated/EventPresentation';
import type { Message } from '../api';
import { EventPresentationSchema, SseMessageDataSchema } from '../sseSchemas';
import { ConversationEventCard, eventCardFragments } from './ConversationEventCard';
import { buildConversationSearchProjection } from './viewer-find/searchProjections';
import { copyToClipboard } from '../utils/clipboard';

vi.mock('../utils/clipboard', () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));
const available = {
  status: 'available', event_id: 'event-exact', product_conversation_id: 'product-1', display_name: 'Fix routing',
  source_transcript_id: 'transcript-exact', source_occurrence_kind: 'turn', source_occurrence_id: 'occurrence-exact',
  source_generation: 7, occurred_at_us: 1791800000123456, provenance: 'subscription', outcome: { kind: 'turn_finished' },
} satisfies EventPresentation;
const original = '[original provider text]\nTask complete is not inferred.';
function Location() {
  const location = useLocation();
  return <output data-testid="location">{JSON.stringify(location)}</output>;
}
function mount(presentation: EventPresentation | null = available, fragmentId?: string) {
  return render(<MemoryRouter initialEntries={['/global/coordinator?viewer=inspect#message-origin']}>
    <ConversationEventCard presentation={presentation} eventId="event-exact" originalText={original}
      activeHighlight={fragmentId ? { owner: 'message-text', fragmentId, start: 0, end: 5 } : null}>
      <span data-fragment-id="message-text">{original}</span>
    </ConversationEventCard><Location />
  </MemoryRouter>);
}
function message(presentation: EventPresentation | null): Message {
  return { message_id: 'msg', sequence_id: 400, conversation_id: 'global', message_type: 'user',
    origin: { kind: 'subscription_event', event_id: 'event-exact' }, event_presentation: presentation,
    content: { text: original }, created_at: '2026-10-12T00:00:00Z' };
}

describe('compact conversation events', () => {
  it('shows a truthful title, outcome and accessible local occurrence time, not raw IDs', () => {
    const { container } = mount();
    expect(screen.getByRole('link', { name: 'Fix routing' })).toHaveAttribute('href', '/c/product-1');
    expect(screen.getByText('Turn finished')).toBeVisible();
    const date = new Date(available.occurred_at_us / 1000);
    expect(container.querySelector('time')).toHaveAttribute('datetime', date.toISOString());
    expect(container.querySelector('time')).toHaveAccessibleName(date.toLocaleString());
    expect(container.querySelector('details')).not.toHaveAttribute('open');
    expect(container.querySelector('.conversation-event-summary')).not.toHaveTextContent('event-exact');
  });
  it.each([null, '', '   '])('uses an honest untitled label for %s', (display_name) => {
    mount({ ...available, display_name });
    expect(screen.getByRole('link', { name: 'Untitled conversation' })).toBeVisible();
  });
  it.each(['missing', 'lookup_failed', 'redacted'] as const)('does not fabricate a link or outcome when %s', (reason) => {
    mount({ status: 'unavailable', event_id: 'event-exact', reason });
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('Conversation event')).toBeVisible();
    expect(screen.getByText('Details unavailable')).toBeVisible();
    expect(screen.queryByText('Turn finished')).not.toBeInTheDocument();
  });
  it.each(['failed', 'cleanup_failed'] as const)('keeps %s conspicuous and reveals the exact cause', (kind) => {
    const { container } = mount({ ...available, outcome: { kind, reason: 'Exact failure: keep /path?x=1' } }, 'event-reason');
    expect(container.querySelector('.failure')).toBeVisible();
    expect(container.querySelector('details')).toHaveAttribute('open');
    expect(container.querySelector('.conversation-event-reason')).toHaveTextContent('Exact failure: keep /path?x=1');
  });
  it('copies exact provenance and unchanged original text', () => {
    const { container } = mount();
    container.querySelector('details')!.open = true;
    fireEvent.click(screen.getByRole('button', { name: 'Copy event provenance' }));
    expect(copyToClipboard).toHaveBeenCalledWith(eventCardFragments(available, available.event_id).provenance);
    fireEvent.click(screen.getByRole('button', { name: 'Copy original event text' }));
    expect(copyToClipboard).toHaveBeenCalledWith(original);
    expect(container.querySelector('[data-fragment-id="message-text"]')!.textContent).toBe(original);
  });
  it('uses canonical query-safe routing and preserves the complete Coordinator return origin', () => {
    mount({ ...available, product_conversation_id: 'id/?query#hash' });
    const link = screen.getByRole('link', { name: 'Fix routing' });
    expect(link).toHaveAttribute('href', '/c/id%2F%3Fquery%23hash');
    fireEvent.click(link, { ctrlKey: true });
    expect(screen.getByTestId('location')).toHaveTextContent('"pathname":"/global/coordinator"');
    fireEvent.click(link);
    expect(screen.getByTestId('location')).toHaveTextContent('"pathname":"/c/id%2F%3Fquery%23hash"');
    expect(screen.getByTestId('location')).toHaveTextContent('"conversationReturnOrigin":{"kind":"coordinator","href":"/global/coordinator?viewer=inspect#message-origin"}');
  });
  it('retains a long title without truncating its accessible text', () => {
    const title = 'Long title '.repeat(70);
    mount({ ...available, display_name: title });
    expect(screen.getByRole('link')).toHaveTextContent(title.trim());
  });
  it.each(['event-provenance', 'message-text'])('find reveals collapsed %s without new navigation targets', (fragmentId) => {
    const { container } = mount(available, fragmentId);
    expect(container.querySelector('details')).toHaveAttribute('open');
  });
  it.each(['Fix routing', 'Turn finished', 'occurrence-exact', '[original provider text]'])('find indexes %s from typed presentation and original content', (query) => {
    const projection = buildConversationSearchProjection([{ kind: 'user', key: 'msg', message: message(available) }], query);
    expect(projection.matches).toHaveLength(1);
    const source = projection.sources.find(source => source.id === projection.matches[0]!.sourceId)!;
    expect(source.revealTarget?.kind).toBe('message-text');
  });
});

describe('typed event wire retention', () => {
  it.each([400, 401])('retains flattened available payload at sequence %s', (sequence_id) => {
    const parsed = v.parse(SseMessageDataSchema, { sequence_id, message: { ...message(available), sequence_id } });
    expect(parsed.message.event_presentation).toEqual(available);
    expect(parsed.message.content).toEqual({ text: original });
  });
  it.each(['missing', 'lookup_failed', 'redacted'] as const)('retains %s fallback', (reason) => {
    const unavailable = { status: 'unavailable', event_id: 'event-exact', reason } as const;
    expect(v.parse(EventPresentationSchema, unavailable)).toEqual(unavailable);
  });
  it('requires an explicit nullable field and rejects malformed outcomes rather than stripping them', () => {
    const { event_presentation, ...without } = message(null);
    expect(event_presentation).toBeNull();
    expect(v.safeParse(SseMessageDataSchema, { sequence_id: 400, message: without }).success).toBe(false);
    expect(v.safeParse(SseMessageDataSchema, { sequence_id: 400, message: message(null) }).success).toBe(true);
    for (const outcome of [{ kind: 'failed' }, { kind: 'completed' }, { kind: 'cleanup_failed', reason: 5 }]) {
      expect(v.safeParse(EventPresentationSchema, { ...available, outcome }).success).toBe(false);
    }
    expect(v.safeParse(EventPresentationSchema, { ...available, source_generation: '7' }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { status: 'available', payload: available }).success).toBe(false);
  });
});
