import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import * as v from 'valibot';
import type { EventPresentation } from '../generated/EventPresentation';
import type { SseWireEvent } from '../generated/SseWireEvent';
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
const federatedFact = {
  status: 'federated_fact', event_id: 'admission-1', source_instance_id: 'remote-1',
  source_product_conversation_id: 'product-remote', subscription_id: 'subscription-1',
  source_event_id: 'source-event-1', sequence: 8, source_occurrence_kind: 'direct_turn',
  source_occurrence_id: 'occurrence-remote', source_generation: 3, source_transcript_id: 'transcript-remote',
  outcome: 'completed', occurred_at_us: 1791800000123456,
  disposition: { state: 'accepted', transcript_id: 'global-transcript' },
  navigation: { status: 'available', stable_url: 'https://remote.example/c/product-remote', source_url: 'https://remote.example/t/transcript-remote' },
} as const satisfies EventPresentation;
const federatedGap = {
  status: 'federated_gap', event_id: 'admission-gap', source_instance_id: 'remote-1',
  source_product_conversation_id: 'product-remote', subscription_id: 'subscription-1',
  last_acknowledged: 8, last_produced: 14, reason: 'disconnected_age',
  failed_at_us: 1791800000123456, terminal_evidence_expires_at_us: 1794392000123456,
  source_transcript_id: 'transcript-remote', current: { kind: 'terminal', outcome: 'failed' },
  disposition: { state: 'accepted', transcript_id: 'global-transcript' },
  navigation: { status: 'unavailable' },
} as const satisfies EventPresentation;
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
  it('uses the theme link token without inheriting the faded meta-message wrapper', () => {
    const css = readFileSync(`${process.cwd()}/src/components/ConversationEventCard.css`, 'utf8');
    expect(css).toMatch(/\.message\.meta\.conversation-event-message\s*\{\s*opacity:\s*1;/);
    expect(css).toMatch(/\.conversation-event-title\s*\{[^}]*color:\s*var\(--accent-blue\)/);
    expect(css).toMatch(/\.conversation-event-title:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--accent-blue\)/);
  });

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
  it('links an available remote fact through the external stable URL with time and provenance', () => {
    const { container } = mount(federatedFact);
    expect(screen.getByRole('link', { name: 'Remote conversation · remote-1' })).toHaveAttribute('href', federatedFact.navigation.stable_url);
    expect(screen.getByText('Remote completed')).toBeVisible();
    expect(container.querySelector('time')).toHaveAttribute('datetime', new Date(federatedFact.occurred_at_us / 1000).toISOString());
    fireEvent.click(screen.getByText('Event details'));
    expect(screen.getByText(/Source event ID: source-event-1/)).toBeVisible();
    expect(screen.getByText(/Transcript ID: transcript-remote/)).toBeVisible();
    expect(screen.getByText(/Disposition: \{"state":"accepted"/)).toBeVisible();
  });
  it('does not link a remote fact whose navigation is unavailable', () => {
    mount({ ...federatedFact, navigation: { status: 'unavailable' } });
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });
  it('renders a gap as a non-navigable control notification even if navigation claims available', () => {
    const { container } = mount({ ...federatedGap, navigation: federatedFact.navigation });
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('Gap: disconnected age · Terminal (failed)')).toBeVisible();
    expect(container.querySelector('.failure')).toBeVisible();
    expect(container.querySelector('time')).toHaveAttribute('datetime', new Date(federatedGap.failed_at_us / 1000).toISOString());
    fireEvent.click(screen.getByText('Event details'));
    expect(screen.getByText(/Read current state and create a new subscription/)).toBeVisible();
    expect(screen.getByText(/Terminal evidence expires \(microseconds\): 1794392000123456/)).toBeVisible();
    expect(screen.getByText(/Last acknowledged: 8/)).toBeVisible();
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
    const payload = { sequence_id, opening: null, message: { ...message(available), sequence_id } } satisfies Omit<Extract<SseWireEvent, {type: 'message'}>, 'type'>;
    const parsed = v.parse(SseMessageDataSchema, payload);
    expect(parsed.message.event_presentation).toEqual(available);
    expect(parsed.opening).toBeNull();
    expect(parsed.message.content).toEqual({ text: original });
  });
  it.each(['missing', 'lookup_failed', 'redacted'] as const)('retains %s fallback', (reason) => {
    const unavailable = { status: 'unavailable', event_id: 'event-exact', reason } as const;
    expect(v.parse(EventPresentationSchema, unavailable)).toEqual(unavailable);
  });
  it('retains federated facts and gaps through the message wire schema', () => {
    for (const presentation of [federatedFact, federatedGap]) {
      const payload = { sequence_id: 400, opening: null, message: message(presentation) } satisfies Omit<Extract<SseWireEvent, {type: 'message'}>, 'type'>;
      expect(v.parse(SseMessageDataSchema, payload).message.event_presentation).toEqual(presentation);
    }
  });
  it('validates federated nested variants and required provenance', () => {
    expect(v.safeParse(EventPresentationSchema, { ...federatedFact, source_occurrence_kind: 'unknown' }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedFact, navigation: { status: 'available', source_url: 'https://remote.example/t/1' } }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedFact, disposition: { state: 'suppressed', reason: 'unknown' } }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedGap, current: { kind: 'terminal' } }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedGap, terminal_evidence_expires_at_us: null }).success).toBe(false);
  });
  it('requires an explicit nullable field and rejects malformed outcomes rather than stripping them', () => {
    const { event_presentation, ...without } = message(null);
    expect(event_presentation).toBeNull();
    expect(v.safeParse(SseMessageDataSchema, { sequence_id: 400, opening: null, message: without }).success).toBe(false);
    const combined = { sequence_id: 400, opening: null, message: message(null) } satisfies Omit<Extract<SseWireEvent, {type: 'message'}>, 'type'>;
    expect(v.safeParse(SseMessageDataSchema, combined).success).toBe(true);
    expect(v.parse(SseMessageDataSchema, combined).message.event_presentation).toBeNull();
    expect(v.parse(SseMessageDataSchema, combined).opening).toBeNull();
    expect(v.safeParse(SseMessageDataSchema, { sequence_id: 400, message: message(null) }).success).toBe(false);
    for (const outcome of [{ kind: 'failed' }, { kind: 'completed' }, { kind: 'cleanup_failed', reason: 5 }]) {
      expect(v.safeParse(EventPresentationSchema, { ...available, outcome }).success).toBe(false);
    }
    expect(v.safeParse(EventPresentationSchema, { ...available, source_generation: '7' }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { status: 'available', payload: available }).success).toBe(false);
  });
});
