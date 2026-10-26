import { describe, it, expect, beforeEach, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { CoordinatorPage } from './CoordinatorPage';
import { COORDINATOR_BRIEFING_PROMPT } from './coordinatorBriefing';
import type { Conversation } from '../api';
import { ViewJourneyLifecycle } from '../hooks/conversationViewLifecycle';

vi.mock('../hooks/conversationViewTelemetry', async importOriginal => {
  const actual = await importOriginal<typeof import('../hooks/conversationViewTelemetry')>();
  return { ...actual, ViewJourneyReporter: class extends actual.ViewJourneyReporter {
    constructor() { super({ sampleRate: 1, send: async () => new Response(null, { status: 204 }) }); }
  } };
});

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    ensureGlobalCoordinator: vi.fn(),
    getConversation: vi.fn(),
    resolveCoordinatorRoute: vi.fn(),
    getCoordinatorAutomaticContinuation: vi.fn(),
    updateCoordinatorAutomaticContinuation: vi.fn(),
    listLiveCoordinatorBashHandles: vi.fn(),
    listActiveCoordinatorWatches: vi.fn(),
    stopLiveCoordinatorBashHandle: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return { ...actual, api: apiMock };
});

vi.mock('./ProductConversationPage', () => ({
  ProductConversationPage: ({
    productId,
    globalShell,
  }: {
    productId?: string;
    globalShell?: {
      composerQuickAction?: { label: string; compactLabel: string; prompt: string };
      stateBarExtension?: { summary: React.ReactNode; details: React.ReactNode };
    };
  }) => (
    <div>
      Shared conversation runtime /global <span data-testid="history-reference">{productId}</span>
      <div data-testid="mock-statebar">
        {globalShell?.stateBarExtension?.summary}
        {globalShell?.stateBarExtension?.details}
      </div>
      {globalShell?.composerQuickAction && (
        <button type="button" aria-label={globalShell.composerQuickAction.label} data-prompt={globalShell.composerQuickAction.prompt}>
          {globalShell.composerQuickAction.compactLabel}
          <span className="sr-only">{globalShell.composerQuickAction.label}</span>
        </button>
      )}
    </div>
  ),
}));

function renderPage(initialEntry = '/global/conv-coordinator') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route path="/global/:slug?" element={<CoordinatorPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

function CurrentPath() {
  const location = useLocation();
  return <><div>{`${location.pathname}${location.search}${location.hash}`}</div><output data-testid="location-state">{JSON.stringify(location.state)}</output></>;
}

const coordinatorConversation = (): Conversation => ({
  id: 'conv-coordinator',
  slug: 'coordinator',
  title: 'Coordinator',
  model: 'claude-3-5-sonnet',
  cwd: '/coordinator',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 3,
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: 'global:',
});

describe('CoordinatorPage', () => {
  beforeEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    apiMock.ensureGlobalCoordinator.mockResolvedValue({ conversation: coordinatorConversation() });
    apiMock.resolveCoordinatorRoute.mockResolvedValue({ coordinator_id: 'conv-coordinator' });
    apiMock.getConversation.mockImplementation(async (id: string) => ({ conversation: { id } }));
    apiMock.listLiveCoordinatorBashHandles.mockResolvedValue([]);
    apiMock.listActiveCoordinatorWatches.mockResolvedValue([]);
    apiMock.stopLiveCoordinatorBashHandle.mockResolvedValue(undefined);
    apiMock.getCoordinatorAutomaticContinuation.mockResolvedValue({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-product' },
      auto_continue_on_context_exhaustion: false,
      admission: null,
    });
    apiMock.updateCoordinatorAutomaticContinuation.mockResolvedValue({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-product' },
      auto_continue_on_context_exhaustion: true,
      admission: null,
    });
  });

  it('shows the current server-backed watch inventory with readable names and secondary IDs', async () => {
    apiMock.listActiveCoordinatorWatches.mockResolvedValue([{
      product_conversation_id: 'product-readable',
      transcript_id: 'transcript-readable',
      transcript_slug: 'fix-readable-target',
      display_name: 'Fix readable target',
      project_path: '/repo/phoenix',
      state: 'idle',
    }]);
    renderPage();
    const activity = (await screen.findByText(/Watching [\d…]/)).closest('.global-statebar-summary')!;
    await waitFor(() => expect(activity).toHaveTextContent('Watching 1'));
    expect(await screen.findByRole('region', { name: 'Active watches' })).toBeInTheDocument();

    expect(screen.getByRole('link', { name: 'Fix readable target' })).toHaveAttribute('href', '/c/product-readable');
    expect(screen.getByRole('link', { name: 'current transcript' })).toHaveAttribute('href', '/c/fix-readable-target');
    expect(screen.getByTitle('ProductConversation ID')).toHaveTextContent('product-readable');
  });

  it('removes ended watches when the current server inventory refreshes', async () => {
    apiMock.listActiveCoordinatorWatches
      .mockResolvedValueOnce([{
        product_conversation_id: 'product-ended', transcript_id: 'transcript-ended', transcript_slug: null,
        display_name: 'Ending watch', project_path: null, state: 'idle',
      }])
      .mockResolvedValue([]);
    renderPage();
    expect(await screen.findByRole('link', { name: 'Ending watch' })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Active watches' })).not.toBeInTheDocument(), { timeout: 3_000 });
  });

  it('retains the last successful watch inventory and reports refresh failure', async () => {
    apiMock.listActiveCoordinatorWatches
      .mockResolvedValueOnce([{
        product_conversation_id: 'product-retained', transcript_id: 'transcript-retained', transcript_slug: null,
        display_name: 'Retained watch', project_path: null, state: 'idle',
      }])
      .mockRejectedValue(new Error('offline'));
    renderPage();
    expect(await screen.findByRole('link', { name: 'Retained watch' })).toBeInTheDocument();
    expect(await screen.findByRole('status', {}, { timeout: 3_000 })).toHaveTextContent('Could not refresh active watches');
    expect(screen.getByRole('link', { name: 'Retained watch' })).toBeInTheDocument();
  });

  it('shows only server-reported live Coordinator commands with inspect navigation', async () => {
    apiMock.listLiveCoordinatorBashHandles.mockResolvedValue([{
      handle_id: 'b-live', command: 'pnpm test', label: 'UI tests', cwd: '/repo/ui', started_at_ms: 123, can_stop: true,
    }]);

    let validationReleased = false;
    const pendingValidations: Array<(value: { conversation: { id: string } }) => void> = [];
    apiMock.getConversation
      .mockResolvedValueOnce({ conversation: { id: 'source-1' } })
      .mockImplementation((id: string) => validationReleased
        ? Promise.resolve({ conversation: { id } })
        : new Promise(resolve => { pendingValidations.push(resolve); }));
    render(<MemoryRouter initialEntries={['/global/conv-coordinator?source_transcript=source-1&source_tool=tool-1#message-source']}><Routes>
      <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
    </Routes></MemoryRouter>);

    await screen.findByText('/global?source_transcript=source-1&source_tool=tool-1#message-source');
    await waitFor(() => expect(screen.getByText(/Running 1/).closest('.global-statebar-summary')).toHaveTextContent('Running 1'));
    await waitFor(() => expect(screen.getByText('UI tests').closest('.coordinator-conversation')).toHaveAttribute('hidden'));
    expect(pendingValidations.length).toBeGreaterThan(0);
    expect(screen.getByText('output →')).toHaveAttribute('href', '/global?source_transcript=source-1&source_tool=tool-1&viewer=inspect&handle=b-live#message-source');
    expect(screen.queryByRole('link', { name: 'output →' })).not.toBeInTheDocument();
    await act(async () => {
      validationReleased = true;
      for (const release of pendingValidations) release({ conversation: { id: 'source-1' } });
    });
    const output = await screen.findByRole('link', { name: 'output →' });

    await waitFor(() => expect(screen.getByText(/Running 1/).closest('.global-statebar-summary')).toHaveTextContent('Running 1'));
    const running = await screen.findByRole('region', { name: 'Running commands' });
    expect(running).toHaveTextContent('UI tests');
    expect(running).toHaveTextContent('pnpm test');
    expect(running).toHaveTextContent('Started');
    const commandDetails = screen.getAllByText('Details').at(-1)?.closest('details');
    expect(commandDetails).not.toHaveAttribute('open');
    fireEvent.click(commandDetails!.querySelector('summary')!);
    expect(running).toHaveTextContent('/repo/ui');
    expect(running).toHaveTextContent('b-live');
    expect(output).toHaveAttribute('href', '/global?source_transcript=source-1&source_tool=tool-1&viewer=inspect&handle=b-live#message-source');
    fireEvent.click(screen.getByRole('button', { name: 'stop' }));
    await waitFor(() => expect(apiMock.stopLiveCoordinatorBashHandle).toHaveBeenCalledWith('b-live'));
    expect(screen.getByRole('region', { name: 'Running commands' })).toBeInTheDocument();
  });

  it('wires the active check-in through the shared conversation runtime', async () => {
    renderPage();

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    const action = screen.getByRole('button', { name: 'Check in & drive' });
    expect(action).toHaveAttribute('data-prompt', COORDINATOR_BRIEFING_PROMPT);
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('including idle owners, but intervene selectively');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Leave healthy progressing work alone');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('do not assume idle owners are stalled');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('do not expand authority or reset prior decisions, pauses or gates');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Recover unanswered user decisions from history');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('verify each is still unresolved and material');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Advance independent authorized work first');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('ask_user_question with context and a recommendation');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Brief me first on what still needs my input');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('the problem, what you did, and where delivery goes next');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('not a fixed report template');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('through existing owners within verified standing authority');
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Do not wait indefinitely for owner replies');
    expect(COORDINATOR_BRIEFING_PROMPT).not.toContain('Do not send messages');

    expect(screen.queryByRole('heading', { name: 'Coordinator' })).not.toBeInTheDocument();
    expect(screen.queryByRole('tablist', { name: 'Coordinator view' })).not.toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Coordinator sections' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Coordinator work')).not.toBeInTheDocument();
    expect(screen.queryByText('Current work context is attached to each Coordinator message.')).not.toBeInTheDocument();
  });

  it('loads and immediately persists the Global Coordinator automatic-continuation toggle', async () => {
    renderPage();

    const control = await screen.findByTestId('automatic-continuation-control');
    expect(screen.getByTestId('mock-statebar')).toContainElement(control);
    fireEvent.click(control.querySelector('summary')!);
    const checkbox = screen.getByRole('checkbox', { name: 'Automatically accept future generated handoffs and continue' });
    await waitFor(() => {
      expect(apiMock.getCoordinatorAutomaticContinuation).toHaveBeenCalledTimes(1);
      expect(checkbox).toBeEnabled();
      expect(checkbox).not.toBeChecked();
    });

    fireEvent.click(checkbox);
    await waitFor(() => expect(apiMock.updateCoordinatorAutomaticContinuation).toHaveBeenCalledWith(true));
    expect(await screen.findByText('Saved')).toBeInTheDocument();
  });

  it('marks bootstrap loading and errors for overlay placement', async () => {
    let resolveCoordinator!: (value: { conversation: Conversation }) => void;
    apiMock.ensureGlobalCoordinator.mockReturnValueOnce(new Promise((resolve) => {
      resolveCoordinator = resolve;
    }));

    const { unmount } = renderPage();
    expect(screen.getByText('Loading…')).toHaveClass('coordinator-page-status');
    resolveCoordinator({ conversation: coordinatorConversation() });
    await screen.findByText('Shared conversation runtime /global');
    unmount();

    apiMock.ensureGlobalCoordinator.mockRejectedValueOnce(new Error('Coordinator unavailable'));
    renderPage();
    expect(await screen.findByText('Coordinator unavailable')).toHaveClass('coordinator-page-status');
  });

  it('redirects an ordinary conversation away from the Coordinator shell', async () => {
    apiMock.resolveCoordinatorRoute.mockResolvedValueOnce({ coordinator_id: null });
    render(
      <MemoryRouter initialEntries={['/global/ordinary-conversation']}>
        <Routes>
          <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('/global/conv-coordinator')).toBeInTheDocument();
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith('ordinary-conversation');
  });

  it('canonicalizes a historical Coordinator chain member without exposing aggregate controls', async () => {
    render(
      <MemoryRouter initialEntries={['/global/old-coordinator?view=history#message-source']}>
        <Routes>
          <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    expect(screen.getByText('/global/old-coordinator?view=history#message-source')).toBeInTheDocument();
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith('old-coordinator');
    expect(await screen.findByTestId('automatic-continuation-control')).toBeInTheDocument();
  });

  it.each(['old-member', 'conv-coordinator'])('validates and selects direct Global query member %s', async (pin) => {
    render(<MemoryRouter initialEntries={[`/global/conv-coordinator?source_transcript=${pin}&viewer=inspect#message-old%3Amsg`]}><Routes>
      <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
    </Routes></MemoryRouter>);
    await screen.findByText('Shared conversation runtime /global');
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith(pin);
    expect(await screen.findByText(`${pin === 'conv-coordinator' ? '/global/conv-coordinator' : '/global'}?source_transcript=${pin}&viewer=inspect#message-old%3Amsg`)).toBeInTheDocument();
  });

  it('retains the same acquired-history owner while a different valid source pin is validated', async () => {
    function SelectOtherSource() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/global/old-member?source_transcript=conv-coordinator#message-new')}>select other member</button>;
    }
    render(<MemoryRouter initialEntries={['/global/old-member?source_transcript=old-member#message-old']}><Routes>
      <Route path="/global/:slug?" element={<><CoordinatorPage /><SelectOtherSource /><CurrentPath /></>} />
    </Routes></MemoryRouter>);
    const owner = await screen.findByTestId('history-reference');
    expect(owner).toHaveTextContent('old-member');
    let release!: (value: { conversation: { id: string } }) => void;
    apiMock.getConversation.mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    fireEvent.click(screen.getByRole('button', { name: 'select other member' }));
    expect(screen.getByTestId('history-reference')).toBe(owner);
    await waitFor(() => expect(apiMock.getConversation).toHaveBeenCalledWith('conv-coordinator'));
    await act(async () => release({ conversation: { id: 'conv-coordinator' } }));
    await waitFor(() => expect(owner.closest('section')).not.toHaveAttribute('hidden'));
    expect(screen.getByTestId('history-reference')).toBe(owner);
    expect(owner).toHaveTextContent('old-member');
    expect(screen.getByText('/global/old-member?source_transcript=conv-coordinator#message-new')).toBeInTheDocument();
  });

  it('does not fail a replacement journey with the preceding source selection error', async () => {
    const finish = vi.spyOn(ViewJourneyLifecycle.prototype, 'finish');
    function SelectValid() {
      const navigate = useNavigate();
      return <button onClick={() => navigate('/global/conv-coordinator?source_transcript=conv-coordinator')}>select valid</button>;
    }
    render(<MemoryRouter initialEntries={['/global/conv-coordinator?source_transcript=']}><Routes>
      <Route path="/global/:slug?" element={<><CoordinatorPage /><SelectValid /></>} />
    </Routes></MemoryRouter>);
    await screen.findByText('Original source conversation unavailable');
    finish.mockClear();
    let release!: (value: { conversation: Conversation }) => void;
    apiMock.ensureGlobalCoordinator.mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    fireEvent.click(screen.getByText('select valid'));
    expect(finish.mock.calls.filter(([outcome]) => outcome === 'failed')).toHaveLength(0);
    expect(screen.queryByText('Original source conversation unavailable')).not.toBeInTheDocument();
    await act(async () => release({ conversation: coordinatorConversation() }));
    await screen.findByTestId('history-reference');
    expect(finish.mock.calls.filter(([outcome]) => outcome === 'failed')).toHaveLength(0);
    finish.mockRestore();
  });

  it.each(['café', 'caf%C3%A9', '%', '%25', 'old%2Frow', 'old%2frow', 'x/y?source_transcript=evil', '[x]', '.', '..'])('admits exact opaque Global query member without a path rewrite: %s', async pin => {
    const href = `/global?source_transcript=${encodeURIComponent(pin)}&viewer=inspect#message-${encodeURIComponent(pin)}`;
    const url = new URL(href, 'https://phoenix.test');
    const state = { conversationReturnOrigin: { kind: 'coordinator', href: '/global/origin?x=1#message-original' } };
    render(<MemoryRouter initialEntries={[{ pathname: url.pathname, search: url.search, hash: url.hash, state }]}><Routes>
      <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
    </Routes></MemoryRouter>);
    expect(await screen.findByTestId('history-reference')).toHaveTextContent(pin);
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith('conv-coordinator');
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith(pin);
    expect(apiMock.getConversation).toHaveBeenCalledWith(pin);
    expect(screen.getByText(href)).toBeInTheDocument();
    expect(JSON.parse(screen.getByTestId('location-state').textContent!)).toEqual(state);
  });

  it.each(['old%2Frow', 'old%2frow', 'café', 'caf%C3%A9', 'x/y?z=1'])('decodes Global path reference exactly once at the receiving boundary: %s', async id => {
    renderPage(`/global/${encodeURIComponent(id)}?source_transcript=${encodeURIComponent(id)}`);
    expect(await screen.findByTestId('history-reference')).toHaveTextContent(id);
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith(id);
    expect(apiMock.getConversation).toHaveBeenCalledWith(id);
  });

  it.each(['', 'x&source_transcript=x', 'bad%20space', 'bad%23hash', '%ZZ', '%FF'])('fails closed at the query-only Global entry for malformed pin %s', async pin => {
    renderPage(`/global?source_transcript=${pin}`);
    await screen.findByText('Original source conversation unavailable');
    expect(apiMock.getConversation).not.toHaveBeenCalled();
    expect(screen.queryByTestId('history-reference')).toBeNull();
  });

  it.each(['missing', 'nonmember', 'slug', 'unrelated-reference'])('does not substitute current Global for %s exact source', async failure => {
    apiMock.resolveCoordinatorRoute.mockImplementation(async (id: string) => ({
      coordinator_id: id === 'unrelated' || (failure === 'nonmember' && id === 'exact%2Fpin') ? null : 'conv-coordinator',
    }));
    apiMock.getConversation.mockImplementation(async (id: string) => {
      if (failure === 'missing') throw new Error('Not found');
      return { conversation: { id: failure === 'slug' ? 'different-id' : id } };
    });
    renderPage(`${failure === 'unrelated-reference' ? '/global/unrelated' : '/global'}?source_transcript=exact%252Fpin`);
    await screen.findByText(failure === 'missing' ? 'Not found' : 'Original source conversation unavailable');
    expect(screen.queryByTestId('history-reference')).toBeNull();
  });

  it('rejects a slug pin even when it belongs to the Global domain', async () => {
    apiMock.getConversation.mockResolvedValue({ conversation: { id: 'actual-transcript-id' } });
    renderPage('/global/conv-coordinator?source_transcript=friendly-slug');
    await screen.findByText('Original source conversation unavailable');
    expect(screen.queryByText('Shared conversation runtime /global')).toBeNull();
  });

  it.each(['', 'foreign', 'old&source_transcript=other'])('fails closed on invalid direct Global pin %s', async (pin) => {
    apiMock.resolveCoordinatorRoute.mockImplementation(async (reference: string) => ({ coordinator_id: reference === 'conv-coordinator' ? 'conv-coordinator' : null }));
    renderPage(`/global/conv-coordinator?source_transcript=${pin}`);
    await screen.findByText('Original source conversation unavailable');
    expect(screen.queryByText('Shared conversation runtime /global')).toBeNull();
  });

  it('keeps an unavailable exact source member in the Coordinator error layout', async () => {
    apiMock.resolveCoordinatorRoute.mockResolvedValueOnce({ coordinator_id: null });
    renderPage('/global/missing-source?source_tool=call#message-source');
    expect(await screen.findByText('Original source conversation unavailable')).toHaveClass('coordinator-page-status');
    expect(screen.queryByText('Shared conversation runtime /global')).not.toBeInTheDocument();
  });

  it('replaces a stale Coordinator continuation URL with the singleton route', async () => {
    apiMock.resolveCoordinatorRoute.mockResolvedValueOnce({ coordinator_id: null });
    render(
      <MemoryRouter initialEntries={['/global/stale-coordinator']}>
        <Routes>
          <Route path="/global/:slug?" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('/global/conv-coordinator')).toBeInTheDocument();
  });
});
