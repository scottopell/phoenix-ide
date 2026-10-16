import { afterEach, expect, it, vi } from 'vitest';
import { ViewJourneyReporter, type ViewIdentity } from './conversationViewTelemetry';
import { emitViewDiagnostic } from './conversationViewDiagnostics';
const identity: ViewIdentity = {
  product_conversation_id: null, segment_id: null, route: 'ordinary', trigger: 'navigation', standalone: false,
  platform: 'desktop', frontend_source: null, frontend_source_dirty: false, cache: 'unknown', message_count_bucket: 'unknown',
  chapter_count_bucket: 'unknown', response_size_bucket: 'unknown', reconnect_count: 0, visible: true,
};
afterEach(() => vi.restoreAllMocks());
it('emits a real unsampled decision with no report despite independent legacy request identity', () => {
  const dispatch = vi.spyOn(window, 'dispatchEvent');
  const send = vi.fn<typeof fetch>();
  expect(new ViewJourneyReporter({ random: () => 0.9, send }).begin(identity)).toBeNull();
  expect((dispatch.mock.calls[0]![0] as CustomEvent).detail).toEqual({ version: 1, kind: 'admission', route: 'ordinary', open_id: null, sampled: false });
  expect(send).not.toHaveBeenCalled();
});
it('records partial marks without inventing readiness or submitting a stalled sampled journey', () => {
  const dispatch = vi.spyOn(window, 'dispatchEvent');
  const send = vi.fn<typeof fetch>();
  const attempt = new ViewJourneyReporter({ random: () => 0.1, send }).begin(identity, () => 0)!;
  attempt.measurement.mark('shell'); attempt.measurement.mark('shell');
  const events = dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail);
  expect(events).toEqual([
    { version: 1, kind: 'admission', route: 'ordinary', open_id: attempt.openId, sampled: true },
    { version: 1, kind: 'mark', mark: 'shell', ms: 0, open_id: attempt.openId },
  ]);
  expect(send).not.toHaveBeenCalled();
});
it('terminal then attempted then failed remain distinct and ordered for genuine rejected transport', async () => {
  const dispatch = vi.spyOn(window, 'dispatchEvent');
  const send = vi.fn<typeof fetch>().mockRejectedValue(new Error('PRIVATE_NETWORK_ERROR'));
  const reporter = new ViewJourneyReporter({ random: () => 0.1, send });
  reporter.report(reporter.begin(identity)!, 'incomplete');
  await vi.waitFor(() => expect(dispatch.mock.calls.some(([event]) => (event as CustomEvent).detail.state === 'failed')).toBe(true));
  const events = dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail);
  expect(events.map(e => e.kind === 'submission' ? e.state : e.kind)).toEqual(['admission', 'terminal', 'attempted', 'failed']);
  expect(JSON.stringify(events)).not.toContain('PRIVATE');
  expect(send).toHaveBeenCalledTimes(1);
});
it('diagnostic dispatch failure cannot prevent admission or transport', async () => {
  vi.spyOn(window, 'dispatchEvent').mockImplementation(() => { throw new Error('listener broken'); });
  const send = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
  const reporter = new ViewJourneyReporter({ random: () => 0.1, send });
  reporter.report(reporter.begin(identity)!, 'failed');
  await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
  expect(() => emitViewDiagnostic({ kind: 'admission', route: 'ordinary', open_id: null, sampled: false })).not.toThrow();
});

it('initiates injected keepalive fetch within the pagehide handler rather than a teardown microtask', () => {
  const order: string[] = [];
  const send = vi.fn<typeof fetch>().mockImplementation((_url, init) => {
    order.push('fetch');
    expect(init?.keepalive).toBe(true);
    expect(init?.signal?.aborted).toBe(false);
    return Promise.resolve(new Response(null, { status: 204 }));
  });
  const reporter = new ViewJourneyReporter({ random: () => 0.1, send });
  const attempt = reporter.begin(identity)!;
  const onPagehide = () => { reporter.report(attempt, 'incomplete'); order.push('handler_return'); };
  window.addEventListener('pagehide', onPagehide);
  try {
    window.dispatchEvent(new Event('pagehide'));
    expect(order).toEqual(['fetch', 'handler_return']);
    expect(send).toHaveBeenCalledTimes(1);
  } finally { window.removeEventListener('pagehide', onPagehide); }
});

it('isolates synchronous fetch throws, records failed transport once and does not echo error text', async () => {
  const dispatch = vi.spyOn(window, 'dispatchEvent');
  const send = vi.fn<typeof fetch>().mockImplementation(() => { throw new Error('PRIVATE_SYNC_FAILURE'); });
  const reporter = new ViewJourneyReporter({ random: () => 0.1, send });
  expect(() => reporter.report(reporter.begin(identity)!, 'incomplete')).not.toThrow();
  await vi.waitFor(() => expect(dispatch.mock.calls.some(([event]) => (event as CustomEvent).detail.state === 'failed')).toBe(true));
  expect(send).toHaveBeenCalledTimes(1);
  expect(JSON.stringify(dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail))).not.toContain('PRIVATE');
});

it('distinguishes the sole timeout abort from rejected transport without extending its deadline', async () => {
  vi.useFakeTimers();
  try {
    const dispatch = vi.spyOn(window, 'dispatchEvent');
    const send = vi.fn<typeof fetch>().mockImplementation((_url, init) => new Promise((_resolve, reject) => {
      init!.signal!.addEventListener('abort', () => reject(new Error('PRIVATE_ABORT')));
    }));
    const reporter = new ViewJourneyReporter({ random: () => 0.1, send });
    reporter.report(reporter.begin(identity)!, 'incomplete');
    await vi.advanceTimersByTimeAsync(2999);
    expect(dispatch.mock.calls.some(([event]) => (event as CustomEvent).detail.state === 'timeout_abort')).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const states = dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail).filter(e => e.kind === 'submission').map(e => e.state);
    expect(states).toEqual(['attempted', 'timeout_abort', 'failed']);
    expect(send).toHaveBeenCalledTimes(1);
  } finally { vi.useRealTimers(); }
});
