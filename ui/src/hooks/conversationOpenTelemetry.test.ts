import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ConversationOpenMeasurement, reportConversationOpen } from './conversationOpenTelemetry';

describe('ConversationOpenMeasurement', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));
  });

  it('separates native open, init transit, and handler duration exactly', () => {
    const times = [100, 125, 144.25, 219.25];
    const measurement = new ConversationOpenMeasurement('open-1', 2, () => times.shift()!);
    measurement.nativeOpen();
    measurement.initReceived();

    expect(measurement.connected()).toMatchObject({
      open_id: 'open-1',
      outcome: 'connected',
      native_open_ms: 25,
      init_received_ms: 44.3,
      handler_ms: 75,
      total_ms: 119.3,
      retry_attempt: 2,
    });
    expect(measurement.connected()).toBeNull();
  });

  it('reports an error before native open without fabricating phase durations', () => {
    const times = [10, 80];
    const measurement = new ConversationOpenMeasurement('open-2', 0, () => times.shift()!);
    expect(measurement.error()).toMatchObject({
      outcome: 'error',
      native_open_ms: null,
      init_received_ms: null,
      handler_ms: null,
      total_ms: 70,
    });
  });

  it('reports cancellation once and bounds unknown network types', () => {
    vi.stubGlobal('navigator', {
      ...navigator,
      connection: { effectiveType: 'attacker-controlled-value' },
    });
    const times = [10, 25];
    const measurement = new ConversationOpenMeasurement('open-canceled', 3, () => times.shift()!);

    expect(measurement.canceled()).toMatchObject({
      outcome: 'canceled',
      retry_attempt: 3,
      effective_type: null,
      total_ms: 15,
    });
    expect(measurement.error()).toBeNull();
  });

  it('preserves additive phases when the shared timeline saturates', () => {
    const times = [0, 299_999, 400_000];
    const measurement = new ConversationOpenMeasurement(
      'open-saturated',
      20_000,
      () => times.shift()!,
    );
    measurement.initReceived();

    expect(measurement.connected()).toMatchObject({
      init_received_ms: 299_999,
      handler_ms: 1,
      total_ms: 300_000,
      retry_attempt: 10_000,
    });
  });

  it('posts a bounded content-free payload with keepalive', () => {
    const payload = {
      open_id: 'open-3',
      outcome: 'connected' as const,
      native_open_ms: 20,
      init_received_ms: 40,
      handler_ms: 1,
      total_ms: 41,
      retry_attempt: 0,
      visible: true,
      effective_type: '4g',
    };
    reportConversationOpen(payload);
    expect(fetch).toHaveBeenCalledWith('/api/telemetry/conversation-open', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
      keepalive: true,
    });
  });
});
