import { act, cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { EmbeddedConversationPage } from './ConversationPage';
import { ConversationProvider } from '../conversation/ConversationProvider';
import { ApplicationCapabilitiesContext } from '../contexts/ApplicationCapabilitiesContext';
import { persistedCapabilities, persistedObservationFixture } from '../testing/persistedObservationFixture';
import { api } from '../api';
import * as modelsPoller from '../modelsPoller';
import { PersistedMemberResponseSchema } from '../observationSchemas';
import * as v from 'valibot';

const transcript = vi.fn();
vi.mock('../components/ConversationNavStack', () => ({
  ConversationNavStack: (props: Record<string, unknown>) => {
    transcript(props);
    return <div data-testid="saved-transcript">{JSON.stringify(props['messages'])}</div>;
  },
}));
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); transcript.mockClear(); });

function owner(slug = 'saved-member') {
  return <ApplicationCapabilitiesContext.Provider value={persistedCapabilities}>
    <ConversationProvider><MemoryRouter>
      <EmbeddedConversationPage slug={slug} />
    </MemoryRouter></ConversationProvider>
  </ApplicationCapabilitiesContext.Provider>;
}

describe('persisted conversation ownership', () => {
  it('decodes persisted data and mounts no live stream, provider polling, or input-command owner', async () => {
    const payload = persistedObservationFixture();
    expect(v.parse(PersistedMemberResponseSchema, payload)).toEqual(payload);
    const fetchMock = vi.fn<typeof fetch>(async () => Response.json(payload));
    vi.stubGlobal('fetch', fetchMock);
    const eventSource = vi.fn(() => { throw new Error('persisted owner must not create EventSource'); });
    vi.stubGlobal('EventSource', eventSource);
    const sendMessage = vi.spyOn(api, 'sendMessage');
    const subscribeModels = vi.spyOn(modelsPoller, 'subscribeModels');
    const addListener = vi.spyOn(window, 'addEventListener');
    render(owner());
    expect(await screen.findByTestId('saved-transcript')).toHaveTextContent('Saved transcript content');
    expect(screen.getByRole('status')).toHaveTextContent('recorded state: awaiting sub agents');
    expect(screen.getByRole('status')).toHaveTextContent('historical, not a live process observation');
    expect(screen.getByRole('status')).toHaveTextContent('newest 1 persisted messages');
    expect(screen.queryByRole('textbox')).toBeNull();
    act(() => {
      window.dispatchEvent(new CustomEvent('phoenix:insert-draft', { detail: { text: 'must not send' } }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
    });
    expect(sendMessage).not.toHaveBeenCalled();
    expect(subscribeModels).not.toHaveBeenCalled();
    expect(addListener.mock.calls.some(([event]) => event === 'phoenix:insert-draft')).toBe(false);
    expect(eventSource).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/conversations/saved-member/observation');
    expect(transcript.mock.lastCall?.[0]).toMatchObject({ transcriptMode: 'persisted', pendingMessages: [], enableMessageSidepanel: false });
  });

  it('ignores a stale read after navigation and does not display the previous member while loading', async () => {
    let resolveOld!: (response: Response) => void;
    const fetchMock = vi.fn().mockImplementationOnce(() => new Promise<Response>((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce(Response.json(persistedObservationFixture()));
    vi.stubGlobal('fetch', fetchMock);
    const view = render(owner('old-member'));
    view.rerender(owner('saved-member'));
    await screen.findByTestId('saved-transcript');
    const old = persistedObservationFixture();
    old.messages[0]!.content = { text: 'stale old content' };
    await act(async () => { resolveOld(Response.json(old)); });
    expect(screen.getByTestId('saved-transcript')).not.toHaveTextContent('stale old content');
    expect(fetchMock.mock.calls[0]?.[1]?.signal.aborted).toBe(true);
  });

  it('surfaces read failures without mounting live fallback ownership', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('', { status: 503 })));
    const eventSource = vi.fn();
    vi.stubGlobal('EventSource', eventSource);
    render(owner());
    expect(await screen.findByRole('alert')).toHaveTextContent('Failed to load persisted conversation');
    expect(screen.queryByTestId('saved-transcript')).toBeNull();
    expect(eventSource).not.toHaveBeenCalled();
  });
});
