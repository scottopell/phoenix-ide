import { afterEach, describe, expect, it, vi } from 'vitest';
import { ViewJourneyReporter, type ViewIdentity } from './conversationViewTelemetry';
const identity: ViewIdentity = {
  product_conversation_id: crypto.randomUUID(), segment_id: crypto.randomUUID(),
  route: 'ordinary', trigger: 'navigation', standalone: false, platform: 'desktop',
  frontend_source: 'b8fc23f', visible: true,
  frontend_source_dirty: false, cache: 'unknown', message_count_bucket: 'unknown', chapter_count_bucket: 'unknown', reconnect_count: 0,
  response_size_bucket: 'unknown',
};

describe('ViewJourneyReporter', () => {
  afterEach(() => vi.useRealTimers());

  it('samples before outcome, retains failed/canceled denominators and creates unique IDs', async () => {
    const send = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
    const random = vi.fn().mockReturnValueOnce(0.9).mockReturnValue(0.1);
    const reporter = new ViewJourneyReporter({ send, random });
    expect(reporter.begin(identity)).toBeNull();
    const failed = reporter.begin(identity)!;
    const canceled = reporter.begin({ ...identity, trigger: 'pwa_resume', route: 'global' })!;
    expect(failed.openId).not.toBe(canceled.openId);
    reporter.report(failed, 'failed');
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const payload = JSON.parse(send.mock.calls[0]![1]!.body as string);
    expect(payload).toMatchObject({ outcome: 'failed', admitted_count: 3, sampled_count: 2, sample_rate: 0.25 });
  });

  it('drops concurrent reports, does not queue offline work, and aborts a single bounded attempt', async () => {
    vi.useFakeTimers();
    const send = vi.fn<typeof fetch>().mockImplementation((_url, init) => new Promise((_resolve, reject) => {
      init!.signal!.addEventListener('abort', () => reject(new Error('offline')));
    }));
    const reporter = new ViewJourneyReporter({ send, sampleRate: 1 });
    reporter.report(reporter.begin(identity)!, 'incomplete');
    reporter.report(reporter.begin(identity)!, 'canceled');
    await vi.advanceTimersByTimeAsync(3000);
    expect(send).toHaveBeenCalledTimes(1);
    send.mockResolvedValue(new Response(null, { status: 204 }));
    reporter.report(reporter.begin(identity)!, 'failed');
    await vi.advanceTimersByTimeAsync(0);
    expect(JSON.parse(send.mock.calls[1]![1]!.body as string)).toMatchObject({
      transport_dropped_count: 1, transport_failed_count: 1,
    });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it('revalidates identity at the final wire boundary after public identity mutation', async () => {
    const send = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
    const reporter = new ViewJourneyReporter({ send, sampleRate: 1 });
    const journey = reporter.begin(identity)!;
    Object.assign(journey.identity, { message_text: 'private transcript', frontend_source: '/private/path' });
    reporter.report(journey, 'canceled');
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0]![1]!.body).not.toMatch(/private|message_text/);
  });

  it('does not spread caller extras, labels, queries, or invalid source identities onto the wire', async () => {
    const send = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
    const reporter = new ViewJourneyReporter({ send, sampleRate: 1 });
    const unsafe = { ...identity, label: 'secret text', query: '?token=secret', frontend_source: '/private/path', segment_id: 'secret' };
    reporter.report(reporter.begin(unsafe)!, 'canceled');
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const body = send.mock.calls[0]![1]!.body as string;
    expect(body).not.toMatch(/secret|private|query|label/);
    expect(JSON.parse(body)).toMatchObject({ segment_id: null, frontend_source: null });
  });
});
