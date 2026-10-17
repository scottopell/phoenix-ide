import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, fireEvent, waitFor } from '@testing-library/react';
import App from '../App';
import { liveCapabilities } from '../contexts/ApplicationCapabilities';
import { getProductConversationScenario, coordinatorSnapshot } from '../fixtures/productConversation/scenarios';
import type { Message } from '../api';

// Keep the real startup, providers, persisted API, history owner and projection.
vi.mock('../components/ConversationNavStack', () => ({
  ConversationNavStack: ({ messages, onLoadOlderMessages }: { messages: Message[]; onLoadOlderMessages?: () => void }) =>
    <div data-testid="persisted-messages">{messages.map(message => <span key={message.message_id} data-message-type={message.message_type}>{message.message_id}</span>)}
      {onLoadOlderMessages && <button onClick={onLoadOlderMessages}>Older</button>}
    </div>,
}));
const liveOwner = vi.hoisted(() => vi.fn());
vi.mock('./ConversationPage', () => ({ EmbeddedConversationPage: () => { liveOwner(); return null; } }));
vi.mock('../components/DesktopLayout', () => ({ DesktopLayout: () => <div>Live application shell</div> }));

const observed = { ...liveCapabilities, live_stream: 'unavailable', conversation_activation: 'unavailable', user_input: 'unavailable', telemetry: 'unavailable', provider_catalog: 'unavailable', host_resource_observations: 'unavailable' };
const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); liveOwner.mockClear(); });
function setup(path: string, capabilities: unknown = observed, kind: 'ordinary' | 'coordinator' = 'ordinary', targetOverride?: unknown) {
  window.history.replaceState({}, '', path);
  const snapshot = getProductConversationScenario('desktop-multi-segment-qa-work').snapshot!;
  const common = {
    product_conversation_id: snapshot.product_conversation_id,
    canonical_route: kind === 'ordinary' ? '/c/persisted' : `/global/${snapshot.latest_transcript_row_id}`,
    canonical_root: snapshot.canonical_root,
    latest_transcript_row_id: snapshot.latest_transcript_row_id, updated_at: snapshot.updated_at,
  };
  const target = targetOverride ?? (kind === 'ordinary'
    ? { kind, conversation: { ...common, lifecycle: { state: 'history' }, presentation: snapshot.presentation } }
    : { kind, conversation: common });
  const requests: string[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input); requests.push(url);
    if (url === '/api/capabilities') return json(capabilities);
    if (url === '/api/auth/status') return json({ auth_required: false, authenticated: true });
    if (url === '/api/observation/target') return json(target);
    if (url === '/api/product-conversations') return json({ product_conversations: [] });
    if (url.includes('boundary_representation=original_messages') || url.includes('/history')) return json(url.startsWith('/api/global/') ? coordinatorSnapshot(snapshot) : snapshot);
    if (url === '/api/conversations') return json([]);
    throw new Error(`Unexpected live request: ${url}`);
  });
  vi.stubGlobal('fetch', fetcher);
  const eventSource = vi.fn(function () { return { close() {}, addEventListener() {}, removeEventListener() {} }; });
  vi.stubGlobal('EventSource', eventSource);
  render(<App />);
  return { requests, snapshot, eventSource };
}
describe('persisted-only application ownership', () => {
  it.each(['/c/persisted', '/product-conversations/persisted', '/global/persisted'])('reads original history without live owners at %s', async path => {
    const { requests, snapshot, eventSource } = setup(path);
    await screen.findByTestId('persisted-messages');
    const latest = snapshot.segments.find(segment => segment.transcript_row_id === snapshot.latest_transcript_row_id)!;
    expect(latest.messages.length).toBeGreaterThan(0);
    for (const message of latest.messages) expect(screen.getByText(message.message_id)).toHaveAttribute('data-message-type', message.message_type);
    expect(screen.getByText(/Input is read-only/)).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /send|activate|close conversation|retry save/i })).not.toBeInTheDocument();
    expect(liveOwner).not.toHaveBeenCalled();
    expect(eventSource).not.toHaveBeenCalled();
    expect(requests[0]).toBe('/api/capabilities');
    expect(requests.slice(1).filter(url => !(url.includes('boundary_representation=original_messages') || url.includes('/history')))).toEqual([]);
    cleanup();
    await Promise.resolve();
    expect(requests.some(url => /auth|models|activate|telemetry|resources|events/.test(url))).toBe(false);
  });
  it('discovers the ordinary target and follows its canonical link', async () => {
    const { requests } = setup('/');
    const link = await screen.findByRole('link', { name: /Product Alpha/ });
    fireEvent.click(link);
    await screen.findByTestId('persisted-messages');
    expect(requests).toContain('/api/observation/target');
    expect(requests.filter(url => !(url === '/api/capabilities' || url === '/api/observation/target' || (url.includes('boundary_representation=original_messages') || url.includes('/history'))))).toEqual([]);
  });
  it.each(['/', '/global'])('discovers Global without an ordinary row at %s', async path => {
    const { requests, snapshot, eventSource } = setup(path, observed, 'coordinator');
    const link = await screen.findByRole('link', { name: snapshot.canonical_root.title! });
    expect(link).toHaveAttribute('href', `/global/${snapshot.latest_transcript_row_id}`);
    fireEvent.click(link);
    await screen.findByTestId('persisted-messages');
    expect(requests).toContain(`/api/global/coordinator/history/${snapshot.latest_transcript_row_id}?message_limit=100`);
    expect(requests.every(url => url === '/api/capabilities' || url === '/api/observation/target' || url.startsWith('/api/global/coordinator/history/'))).toBe(true);
    expect(liveOwner).not.toHaveBeenCalled();
    expect(eventSource).not.toHaveBeenCalled();
  });
  it.each([
    {},
    { kind: 'unknown', conversation: {} },
    { kind: 'ordinary', conversation: { product_conversation_id: 'pc', canonical_route: '/c/pc' } },
    { kind: 'coordinator', conversation: { product_conversation_id: 'pc', canonical_route: '/global/latest', canonical_root: { transcript_row_id: 'root', slug: null, title: null }, latest_transcript_row_id: 42, updated_at: 'now' } },
    { kind: 'coordinator', conversation: { product_conversation_id: 'pc', canonical_route: '/global/latest', canonical_root: { transcript_row_id: 'root', slug: null, title: null }, latest_transcript_row_id: 'latest', updated_at: 'now', presentation: {} } },
  ])('fails closed on a malformed observation target %#', async target => {
    const { requests, eventSource } = setup('/', observed, 'ordinary', target);
    await screen.findByText('Persisted observation target unavailable.');
    expect(requests).toEqual(['/api/capabilities', '/api/observation/target']);
    expect(liveOwner).not.toHaveBeenCalled();
    expect(eventSource).not.toHaveBeenCalled();
  });
  it('fails closed for malformed capabilities before authentication', async () => {
    const { requests } = setup('/', {});
    await screen.findByRole('alert');
    expect(requests).toEqual(['/api/capabilities']);
  });
  it('does not attempt history when persisted reads are unavailable', async () => {
    const { requests } = setup('/c/persisted', { ...observed, persisted_member_read: 'unavailable' });
    await screen.findByText('Persisted conversation reads unavailable.');
    expect(requests).toEqual(['/api/capabilities']);
  });
  it('retains normal authentication and live shell for a live server', async () => {
    const { requests } = setup('/', liveCapabilities);
    await screen.findByText('Live application shell');
    await waitFor(() => expect(requests).toContain('/api/auth/status'));
    expect(requests[0]).toBe('/api/capabilities');
  });
});
