import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
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
  it('copies exact provenance and unchanged original text', async () => {
    const { container } = mount();
    container.querySelector('details')!.open = true;
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy event provenance' })); });
    expect(copyToClipboard).toHaveBeenCalledWith(eventCardFragments(available, available.event_id).provenance);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy original event text' })); });
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

const federatedFact = {
  status: 'federated_fact', event_id: 'federation:peer:fact:event',
  instance_id: '11111111-1111-4111-8111-111111111111', product_conversation_id: 'product-1',
  source_transcript_id: 'transcript-exact', outcome: 'completed', occurred_at_us: available.occurred_at_us,
  navigation: { status: 'available', stable_url: 'https://peer.example/c/product-1', source_url: 'https://peer.example/c/transcript-exact?source_transcript=transcript-exact' },
} satisfies EventPresentation;
const federatedGap = {
  status: 'federated_gap', event_id: 'federation:peer:gap:0:8', instance_id: federatedFact.instance_id,
  product_conversation_id: 'product-1', current_transcript_id: 'successor', requested_after: 0, resume_after: 8,
  current: { kind: 'nonterminal' }, navigation: { status: 'unavailable', reason: 'missing_connection' },
} satisfies EventPresentation;

describe('instance-qualified subscription events', () => {
  it('uses destination HTTPS for colliding local IDs, preserving exact historical navigation', () => {
    const { container } = mount(federatedFact);
    container.querySelector('details')!.open = true;
    expect(screen.getByRole('link', { name: 'Remote conversation · 11111111' })).toHaveAttribute('href', federatedFact.navigation.stable_url);
    expect(screen.getByRole('link', { name: 'Exact destination transcript' })).toHaveAttribute('href', federatedFact.navigation.source_url);
    expect(screen.getByText('If this remote destination is unreachable, verify its saved URL in Settings → Connections.')).toBeVisible();
    expect(container.querySelector('a[href="/c/product-1"]')).toBeNull();
    expect(screen.getByText('Turn finished')).toBeVisible();
    expect(container.querySelector('details')!.textContent).toContain('task completion is not inferred');
    expect(eventCardFragments(federatedFact, '').provenance).toContain(federatedFact.instance_id);
  });
  it('shows an explicit gap and current nonterminal state without inventing a completion or local link', () => {
    const { container } = mount(federatedGap, 'event-provenance');
    expect(screen.getByText('Retention gap')).toBeVisible();
    expect(screen.getByText('Intermediate facts unavailable. Current state: Nonterminal.')).toBeVisible();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(container.querySelector('time')).toBeNull();
    const guidance = screen.getByText('Remote navigation unavailable: no connection is configured. Open Settings → Connections to add one.');
    expect(guidance).toBeVisible();
    expect(guidance.closest('details')).toBeNull();
  });
  it('distinguishes a gap terminal snapshot from an intermediate terminal fact', () => {
    const terminalGap = { ...federatedGap, current: { kind: 'terminal', outcome: 'failed' } } as const;
    expect(v.parse(EventPresentationSchema, terminalGap)).toEqual(terminalGap);
    expect(eventCardFragments(terminalGap, '').reason).toContain('Current state: Failed');
    expect(eventCardFragments(terminalGap, '').outcome).toBe('Retention gap');
  });
  it.each([federatedFact, federatedGap])('retains typed federated wire data at the 400/401 boundary', presentation => {
    for (const sequence_id of [400, 401]) {
      const parsed = v.parse(SseMessageDataSchema, { sequence_id, opening: null, message: { ...message(presentation), sequence_id } });
      expect(parsed.message.event_presentation).toEqual(presentation);
    }
  });
  it.each(['/c/product-1', 'javascript:alert(1)', 'http://peer.example/c/x', 'https://secret@peer.example/c/x'])('rejects unsafe destination navigation %s', stable_url => {
    expect(v.safeParse(EventPresentationSchema, { ...federatedFact, navigation: { ...federatedFact.navigation, stable_url } }).success).toBe(false);
  });
  it('rejects non-advancing gaps, fabricated task outcomes, and missing navigation states', () => {
    expect(v.safeParse(EventPresentationSchema, { ...federatedGap, requested_after: 8 }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedGap, current: { kind: 'terminal', outcome: 'done' } }).success).toBe(false);
    expect(v.safeParse(EventPresentationSchema, { ...federatedFact, navigation: undefined }).success).toBe(false);
  });
  it('retains an explicit redacted state for a federated event instead of exposing source IDs', () => {
    mount({ status: 'unavailable', event_id: federatedFact.event_id, reason: 'redacted' });
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.queryByText('Remote conversation · 11111111')).not.toBeInTheDocument();
  });
});
