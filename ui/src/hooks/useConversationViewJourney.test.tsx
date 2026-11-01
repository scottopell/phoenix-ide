import { StrictMode } from 'react';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useNavigate } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useConversationViewJourney } from './useConversationViewJourney';
import type { ViewJourneyLifecycle } from './conversationViewLifecycle';

let current: ViewJourneyLifecycle;
const send = vi.hoisted(() => vi.fn<typeof fetch>());
vi.mock('./conversationViewTelemetry', async importOriginal => {
  const actual = await importOriginal<typeof import('./conversationViewTelemetry')>();
  return { ...actual, ViewJourneyReporter: class extends actual.ViewJourneyReporter {
    constructor() { super({ sampleRate: 1, send }); }
  } };
});
function Probe() {
  current = useConversationViewJourney('ordinary');
  const navigate = useNavigate();
  return <main ref={current.attach}>
    <button onClick={() => navigate('/c/other')}>Next</button>
    <button onClick={() => navigate(-1)}>Back</button>
  </main>;
}
function mount(strict = false) {
  const view = <MemoryRouter initialEntries={['/c/first']}><Probe /></MemoryRouter>;
  return render(strict ? <StrictMode>{view}</StrictMode> : view);
}
function reports() { return send.mock.calls.map(call => JSON.parse(call[1]!.body as string)); }
async function flush() { await act(async () => { await Promise.resolve(); await Promise.resolve(); }); }
function transition(type: 'pagehide' | 'pageshow') {
  const event = new Event(type);
  Object.defineProperty(event, 'persisted', { value: true });
  act(() => window.dispatchEvent(event));
}
function visibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
  act(() => document.dispatchEvent(new Event('visibilitychange')));
}

describe('router journey admission', () => {
  beforeEach(() => {
    send.mockReset().mockResolvedValue(new Response(null, { status: 204 }));
    vi.spyOn(performance, 'getEntriesByType').mockReturnValue([]);
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    vi.stubGlobal('requestAnimationFrame', vi.fn(() => 1));
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
    vi.stubGlobal('matchMedia', vi.fn(() => ({ matches: false })));
  });
  afterEach(async () => {
    cleanup();
    await flush();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('admits one journey across StrictMode effect replay and cancels it once on unmount', async () => {
    const view = mount(true);
    const id = current.openId;
    await flush();
    expect(current.openId).toBe(id);
    expect(send).not.toHaveBeenCalled();
    view.unmount();
    await flush();
    expect(reports()).toHaveLength(1);
    expect(reports()[0]).toMatchObject({ open_id: id, outcome: 'canceled', admitted_count: 1 });
  });

  it('gives navigation and Back fresh IDs without using raw route data', async () => {
    mount();
    const first = current.openId;
    fireEvent.click(screen.getByText('Next'));
    await flush();
    const next = current.openId;
    expect(next).not.toBe(first);
    fireEvent.click(screen.getByText('Back'));
    await flush();
    expect(current.openId).not.toBe(first);
    expect(current.openId).not.toBe(next);
    current.finish('incomplete');
    await flush();
    expect(reports().map(report => report.trigger)).toEqual(['navigation', 'navigation', 'back_forward']);
    expect(JSON.stringify(reports())).not.toMatch(/\/c\/|first|other/);
  });

  it('labels a browser reload without mixing its clock with server time', async () => {
    const navigation = { name: 'navigation', entryType: 'navigation', startTime: 0, duration: 0, toJSON: () => ({}), type: 'reload' };
    vi.mocked(performance.getEntriesByType).mockReturnValue([navigation]);
    mount();
    current.finish('incomplete');
    await flush();
    expect(reports()[0]).toMatchObject({ trigger: 'reload', outcome: 'incomplete' });
  });

  it('starts a new standalone foreground journey and rejects late callbacks from the prior attempt', async () => {
    vi.mocked(matchMedia).mockReturnValue({ matches: true } as MediaQueryList);
    mount();
    const first = current.openId;
    const request = current.requestStarted(crypto.randomUUID())!;
    visibility('hidden');
    visibility('visible');
    await flush();
    expect(current.openId).not.toBe(first);
    request.failed();
    expect(send).toHaveBeenCalledTimes(1);
    current.finish('incomplete');
    await flush();
    expect(reports()[1]).toMatchObject({ trigger: 'pwa_resume', outcome: 'incomplete', marks: { anchor_settled: null } });
  });

  it('admits exactly one BFCache restoration when pageshow precedes foreground visibility', async () => {
    mount();
    const first = current.openId;
    visibility('hidden');
    transition('pagehide');
    await flush();
    transition('pageshow');
    visibility('visible');
    const restored = current.openId;
    transition('pageshow');
    expect(current.openId).toBe(restored);
    expect(restored).not.toBe(first);
    current.finish('incomplete');
    await flush();
    expect(reports().map(report => report.trigger)).toEqual(['navigation', 'back_forward']);
  });
});
