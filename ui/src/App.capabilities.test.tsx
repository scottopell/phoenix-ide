import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { persistedCapabilities } from './testing/persistedObservationFixture';
import { api, ApiResponseError, type Conversation } from './api';

const newPage = vi.hoisted(() => vi.fn());
vi.mock('./cache', () => ({ cacheDB: { init: vi.fn(async () => {}), getPendingOps: vi.fn(async () => []), getConversation: vi.fn(async () => null), putConversation: vi.fn(async () => {}), getConversations: vi.fn(async () => []), putConversations: vi.fn(async () => {}), deleteConversation: vi.fn(async () => {}) } }));
vi.mock('./pages/NewConversationPage', () => ({ NewConversationPage: () => { newPage(); return <div>New page mounted</div>; } }));
vi.mock('./pages/ConversationListPage', () => ({ ConversationListPage: () => <div>Owner list mounted</div> }));
vi.mock('./pages/LoginPage', () => ({ LoginPage: () => <div>Owner login required</div> }));

class ShareStream {
  static instances: ShareStream[] = [];
  listeners = new Map<string, ((event: MessageEvent) => void)[]>();
  constructor(readonly url: string) { ShareStream.instances.push(this); }
  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  close() {}
  emit(type: string, payload: unknown) {
    for (const listener of this.listeners.get(type) ?? []) listener(new MessageEvent(type, { data: JSON.stringify(payload) }));
  }
}

beforeEach(() => {
  ShareStream.instances = [];
  newPage.mockClear();
  vi.stubGlobal('EventSource', ShareStream);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function open(path: string) {
  window.history.replaceState(null, '', path);
  return render(<App />);
}

describe('application authority route boundary', () => {
  it('boots an authenticated owner through real capabilities and live init into a working composer', async () => {
    const liveCapabilities = Object.fromEntries(Object.keys(persistedCapabilities).map((key) => [key, 'available']));
    let finishAuth!: (response: Response) => void;
    let finishCapabilities!: (response: Response) => void;
    const fetchMock = vi.fn<typeof fetch>(async (url) => {
      if (url === '/api/auth/status') return new Promise<Response>((resolve) => { finishAuth = resolve; });
      if (url === '/api/capabilities') return new Promise<Response>((resolve) => { finishCapabilities = resolve; });
      return Response.json({});
    });
    vi.stubGlobal('fetch', fetchMock);
    const conversation = {
      id: 'live-owner', slug: 'live-owner', model: 'test-model', cwd: '/repo',
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
      message_count: 0, transcript_generation: 1, state: { type: 'idle' }, archived: false,
      browser_session_active: false, terminal_uses_tmux: false,
      conv_mode_label: 'Direct', work_scope_key: 'direct:/repo',
    } as Conversation;
    vi.spyOn(api, 'resolveCoordinatorRoute').mockResolvedValue({ coordinator_id: null });
    vi.spyOn(api, 'getProductConversationSnapshot').mockRejectedValue(new ApiResponseError('not an aggregate', 404));
    vi.spyOn(api, 'getConversationRoute').mockResolvedValue({ id: conversation.id, slug: conversation.slug });
    vi.spyOn(api, 'getConversationRouteBySlug').mockResolvedValue({ id: conversation.id, slug: conversation.slug });
    vi.spyOn(api, 'getConversation').mockResolvedValue({ conversation, messages: [], agent_working: false, presentation_mode: 'idle', context_window_size: 0 });
    vi.spyOn(api, 'listConversations').mockResolvedValue([]);
    vi.spyOn(api, 'listArchivedConversations').mockResolvedValue([]);
    vi.spyOn(api, 'listProductConversations').mockResolvedValue({ product_conversations: [] });
    vi.spyOn(api, 'listModels').mockResolvedValue({ models: [{ id: 'test-model', provider: 'test', description: 'Test model', context_window: 200000, recommended: true }], default: 'test-model', llm_configured: true, credential_status: 'valid' });
    vi.spyOn(api, 'getWorkScopeInventory').mockResolvedValue({ scope_key: 'direct:/repo', bash: [], tmux: null, browser: null });
    vi.spyOn(api, 'listForkProposals').mockResolvedValue([]);
    const send = vi.spyOn(api, 'sendMessage').mockResolvedValue({ queued: false });
    const errors = vi.spyOn(console, 'error');
    open('/c/live-owner');
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/auth/status'));
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(['/api/auth/status']);
    expect(ShareStream.instances).toHaveLength(0);
    expect(screen.queryByPlaceholderText('Type a message...')).toBeNull();
    await act(async () => finishAuth(Response.json({ auth_required: true, authenticated: true })));
    expect(await screen.findByText('Loading application capabilities…')).toBeInTheDocument();
    expect(ShareStream.instances).toHaveLength(0);
    await act(async () => finishCapabilities(Response.json(liveCapabilities)));
    await waitFor(() => expect(ShareStream.instances.some((stream) => stream.url.startsWith('/api/conversations/live-owner/stream'))).toBe(true), { timeout: 10000 });
    const stream = ShareStream.instances.find((entry) => entry.url.startsWith('/api/conversations/live-owner/stream'))!;
    act(() => stream.emit('init', {
      sequence_id: 1, transcript_generation: 1, transcript_coverage: 'complete', conversation,
      messages: [], steering_messages: [], agent_working: false, last_sequence_id: 1,
      stream_incarnation: 'live-test-stream', presentation_mode: 'idle', context_window_size: 0,
      project_name: null, pending_anchor_sequence_id: 0, pending_events: [], pending_truncated: false,
    }));
    const input = await screen.findByPlaceholderText('Type a message...');
    expect(input).toBeEnabled();
    expect(input).not.toHaveAttribute('readonly');
    fireEvent.change(input, { target: { value: 'Positive owner qualification' } });
    fireEvent.keyDown(input, { key: 'Enter', code: 'Enter', shiftKey: false });
    await waitFor(() => expect(send).toHaveBeenCalledWith('live-owner', 'Positive owner qualification', expect.any(Array), expect.any(Array), expect.any(String)));
    expect(fetchMock.mock.calls.filter(([url]) => url === '/api/capabilities')).toHaveLength(1);
    expect(errors).not.toHaveBeenCalled();
  });

  it('renders the real public transcript without owner authentication or capabilities', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => new Response(null, { status: 401 }));
    vi.stubGlobal('fetch', fetchMock);
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(600);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 120, width: 800, height: 120, toJSON: () => ({}) });
    open('/s/public-token');
    await waitFor(() => expect(ShareStream.instances).toHaveLength(1), { timeout: 10000 });
    expect(ShareStream.instances[0]!.url).toBe('/api/share/public-token/events');
    act(() => ShareStream.instances[0]!.emit('init', {
      sequence_id: 1, transcript_generation: 1, transcript_coverage: 'complete',
      conversation: { id: 'shared', slug: 'shared', model: 'recorded' },
      messages: [{ conversation_id: 'shared', message_id: 'shared-message', sequence_id: 1, message_type: 'user', content: { text: 'Public transcript evidence' }, origin: { kind: 'user_api' }, created_at: '2026-01-01T00:00:00Z', display_data: null }],
      steering_messages: [], agent_working: false, last_sequence_id: 1, stream_incarnation: 'share-stream',
      presentation_mode: 'idle', context_window_size: 0, project_name: null,
      pending_anchor_sequence_id: 0, pending_events: [], pending_truncated: false,
    }));
    expect(await screen.findByText('Public transcript evidence')).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
    act(() => {
      window.history.pushState(null, '', '/new');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/auth/status'));
    expect(newPage).not.toHaveBeenCalled();
  });

  it('waits for decoded capabilities and does not mount direct /new when activation is absent', async () => {
    let finish!: (response: Response) => void;
    const fetchMock = vi.fn<typeof fetch>(async (url) => {
      if (url === '/api/auth/status') return Response.json({ auth_required: false, authenticated: true });
      if (url === '/api/capabilities') return new Promise<Response>((resolve) => { finish = resolve; });
      throw new Error(`Unexpected owner request: ${String(url)}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    open('/new');
    expect(await screen.findByText('Loading application capabilities…')).toBeInTheDocument();
    expect(newPage).not.toHaveBeenCalled();
    await act(async () => finish(Response.json(persistedCapabilities)));
    expect(await screen.findByText('Conversation creation unavailable.')).toBeInTheDocument();
    expect(newPage).not.toHaveBeenCalled();
    expect(ShareStream.instances).toHaveLength(0);
  });

  it('does not request owner capabilities before authentication', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => Response.json({ auth_required: true, authenticated: false }));
    vi.stubGlobal('fetch', fetchMock);
    open('/new');
    expect(await screen.findByText('Owner login required')).toBeInTheDocument();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(['/api/auth/status']);
    expect(newPage).not.toHaveBeenCalled();
  });

  it('rejects malformed capabilities without mounting owner pages', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>(async (url) => Response.json(url === '/api/auth/status' ? { auth_required: false, authenticated: true } : {})));
    open('/new');
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load application capabilities');
    expect(newPage).not.toHaveBeenCalled();
  });
});
