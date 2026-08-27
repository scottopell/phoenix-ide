import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ConversationOpenMeasurement, reportConversationOpen } from './conversationOpenTelemetry';

describe('ConversationOpenMeasurement', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));
  });

  it('reports cumulative route-to-paint milestones exactly', () => {
    const times = [100, 110, 120, 125, 144.25, 160, 219.25];
    const measurement = new ConversationOpenMeasurement('open-1', 2, () => times.shift()!);
    measurement.routeResolved();
    measurement.eventSourceCreated();
    measurement.nativeOpen();
    measurement.initReceived();
    measurement.initHandled();

    expect(measurement.firstPaint()).toMatchObject({
      open_id: 'open-1',
      outcome: 'connected',
      route_resolved_ms: 10,
      event_source_created_ms: 20,
      native_open_ms: 25,
      init_received_ms: 44.3,
      init_handled_ms: 60,
      first_paint_ms: 119.3,
      total_ms: 119.3,
      retry_attempt: 2,
    });
    expect(measurement.firstPaint()).toBeNull();
  });

  it('reports an error before native open without fabricating phase durations', () => {
    const times = [10, 80];
    const measurement = new ConversationOpenMeasurement('open-2', 0, () => times.shift()!);
    expect(measurement.error()).toMatchObject({
      outcome: 'error',
      native_open_ms: null,
      init_received_ms: null,
      init_handled_ms: null,
      first_paint_ms: null,
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

  it('bounds the cumulative timeline and retry count', () => {
    const times = [0, 299_999, 299_999, 299_999, 400_000];
    const measurement = new ConversationOpenMeasurement(
      'open-saturated',
      20_000,
      () => times.shift()!,
    );
    measurement.eventSourceCreated();
    measurement.initReceived();
    measurement.initHandled();

    expect(measurement.firstPaint()).toMatchObject({
      init_received_ms: 299_999,
      init_handled_ms: 299_999,
      first_paint_ms: 300_000,
      total_ms: 300_000,
      retry_attempt: 10_000,
    });
  });

  it('marks a timeline non-visible after any hidden interval', () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get');
    visibility.mockReturnValue('visible');
    const times = [0, 1, 2, 3, 4];
    const measurement = new ConversationOpenMeasurement('open-hidden', 0, () => times.shift()!);
    measurement.eventSourceCreated();
    measurement.initReceived();
    visibility.mockReturnValue('hidden');
    measurement.documentHidden();
    visibility.mockReturnValue('visible');
    measurement.initHandled();

    expect(measurement.firstPaint()).toMatchObject({
      outcome: 'connected',
      visible: false,
    });
  });

  it('makes invalid connected payloads unrepresentable', () => {
    // @ts-expect-error connected reports structurally require all completion milestones
    const invalid: import('./conversationOpenTelemetry').ConversationOpenTelemetryPayload = {
      open_id: 'invalid',
      outcome: 'connected',
      route_resolved_ms: null,
      native_open_ms: null,
      total_ms: 1,
      retry_attempt: 0,
      visible: true,
      effective_type: null,
    };
    expect(invalid.outcome).toBe('connected');
  });

  it('posts a bounded content-free payload with keepalive', () => {
    const payload = {
      open_id: 'open-3',
      outcome: 'connected' as const,
      route_resolved_ms: 5,
      event_source_created_ms: 10,
      native_open_ms: 20,
      init_received_ms: 40,
      init_handled_ms: 41,
      first_paint_ms: 45,
      total_ms: 45,
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
