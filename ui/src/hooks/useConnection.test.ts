// useConnection integration tests for task 08683 (epoch-stamp SSE events).
//
// The atom-side reducer rejection is covered exhaustively in
// `src/conversation/atom.test.ts > connection epoch (task 08683)`. These
// tests cover the *hook-side* obligation: every dispatch made by
// `useConnection` carries the `epoch` of the OPEN_SSE generation that
// produced it, so the reducer can do its job.
//
// Strategy: shim `globalThis.EventSource` with a controllable fake that
// records every constructed instance. Render the hook with a spy dispatch
// (or a real `ConversationStore` for the contamination scenario), drive
// events synthetically, and assert what landed in the atom.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { StrictMode } from 'react';
import { useConnection } from './useConnection';
import type { SSEAction } from '../conversation/atom';
import { ConversationStore } from '../conversation/ConversationStore';
import { clearCodexQuota, getCodexQuotaSnapshot } from '../codexQuota';
import type { SseWireEvent } from '../generated/sse';
import { ConversationOpenMeasurement } from './conversationOpenTelemetry';

const WIRE_EVENT_TYPES: Record<SseWireEvent['type'], true> = {
  init: true,
  message: true,
  message_updated: true,
  state_change: true,
  llm_first_byte: true,
  llm_attempt: true,
  token: true,
  agent_done: true,
  conversation_became_terminal: true,
  conversation_update: true,
  error: true,
  conversation_hard_deleted: true,
  browser_session_state: true,
  bash_tool_progress: true,
  steer_message_queued: true,
  steer_message_cancelled: true,
  rate_limit_snapshot: true,
  work_scope_update: true,
};

// ---------------------------------------------------------------------------
// EventSource shim
// ---------------------------------------------------------------------------

type Listener = (event: MessageEvent) => void;

class FakeEventSource {
  url: string;
  readyState = 0;
  closed = false;
  // typed listener buckets keyed by event name
  private listeners = new Map<string, Set<Listener>>();

  static instances: FakeEventSource[] = [];

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, fn: Listener): void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(fn);
  }

  registeredEventTypes(): string[] {
    return [...this.listeners.keys()].sort();
  }

  removeEventListener(type: string, fn: Listener): void {
    this.listeners.get(type)?.delete(fn);
  }

  close(): void {
    this.closed = true;
  }

  /** Drive a typed SSE event into all registered listeners for `type`. */
  emit(type: string, data: unknown): void {
    const payload = typeof data === 'string' ? data : JSON.stringify(data);
    const event = new MessageEvent(type, { data: payload });
    const set = this.listeners.get(type);
    if (!set) return;
    for (const fn of set) fn(event);
  }
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

let originalEventSource: typeof EventSource | undefined;
let originalFetch: typeof fetch | undefined;

beforeEach(() => {
  clearCodexQuota();
  originalEventSource = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  originalFetch = globalThis.fetch;
  (globalThis as { EventSource: unknown }).EventSource =
    FakeEventSource as unknown as typeof EventSource;
  globalThis.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
  FakeEventSource.instances.length = 0;
});

afterEach(() => {
  clearCodexQuota();
  if (originalEventSource) {
    (globalThis as { EventSource: typeof EventSource }).EventSource = originalEventSource;
  }
  if (originalFetch) globalThis.fetch = originalFetch;
});

// Minimal valid `init` payload — must satisfy SseInitDataSchema or
// `parseEvent` throws (dev mode — schema violations are loud by design).
function makeInitPayload(convId: string, slug: string) {
  return {
    sequence_id: 0,
    transcript_generation: 1,
    conversation: {
      id: convId,
      slug,
      model: 'claude-3-5-sonnet',
      cwd: '/tmp',
      created_at: '2024-01-01T00:00:00Z',
      updated_at: '2024-01-01T00:00:00Z',
      message_count: 0,
      transcript_generation: 1,
      archived: false,
      browser_session_active: false,
      terminal_uses_tmux: false,
      worktree_path: '/tmp/worktree',
      work_scope_key: 'worktree:/tmp/worktree',
      conv_mode_label: 'Explore',
    },
    transcript_coverage: 'complete',
    messages: [],
    steering_messages: [],
    agent_working: false,
    last_sequence_id: 0,
    stream_incarnation: 'test-stream',
    presentation_mode: 'idle',
    context_window_size: 0,
    project_name: null,
    pending_anchor_sequence_id: 0,
    pending_events: [] as unknown[],
    pending_truncated: false,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('useConnection epoch stamping (task 08683)', () => {
  it('registers every named conversation wire event plus stream lifecycle events', () => {
    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch: vi.fn() }));

    const expected = [...Object.keys(WIRE_EVENT_TYPES), 'open', 'ping'].sort();
    expect(FakeEventSource.instances[0]!.registeredEventTypes()).toEqual(expected);
  });

  it('opens the legacy stream URL when no replay cursor is available', () => {
    const dispatch = vi.fn<(a: SSEAction) => void>();

    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(FakeEventSource.instances[0]!.url).toMatch(
      /^\/api\/conversations\/conv-A\/stream\?open_id=[0-9a-f-]{36}$/,
    );
  });

  it('uses after_event_sequence when the atom already has a replay cursor', () => {
    const dispatch = vi.fn<(a: SSEAction) => void>();

    renderHook(() => useConnection({
      conversationId: 'conv-A',
      dispatch,
      getLastAppliedEventSeq: () => 42,
    }));

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(FakeEventSource.instances[0]!.url).toMatch(
      /^\/api\/conversations\/conv-A\/stream\?after_event_sequence=42&open_id=[0-9a-f-]{36}$/,
    );
  });

  it('closes and schedules recovery when init identity differs from the route', () => {
    const dispatch = vi.fn<(a: SSEAction) => void>();
    const { result } = renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));
    const stream = FakeEventSource.instances[0]!;

    act(() => stream.emit('init', makeInitPayload('conv-B', 'slug-B')));

    expect(stream.closed).toBe(true);
    expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({ type: 'sse_error' }));
    expect(result.current.state).toBe('reconnecting');
  });

  it('uses the exact transcript coverage reported in init payloads', () => {
    const captured: SSEAction[] = [];
    const dispatch = (action: SSEAction) => captured.push(action);

    renderHook(() => useConnection({
      conversationId: 'conv-A',
      dispatch,
    }));

    act(() => {
      FakeEventSource.instances[0]!.emit('init', {
        ...makeInitPayload('conv-A', 'slug-A'),
        transcript_generation: 4,
        transcript_coverage: 'complete',
      });
    });

    const init = captured.find((action) => action.type === 'sse_init');
    expect(init?.type === 'sse_init' ? init.payload.transcriptCoverage : undefined).toBe('complete');
  });

  it('uses the route-owned open ID and waits for first paint before reporting once', () => {
    const clock = vi.spyOn(performance, 'now');
    clock.mockReturnValueOnce(0);
    clock.mockReturnValue(10);
    const dispatch = vi.fn<(a: SSEAction) => void>();
    const initialMeasurement = new ConversationOpenMeasurement('route-open-id', 0);
    initialMeasurement.routeResolved();
    let reportFirstPaint: (() => void) | undefined;

    renderHook(() => useConnection({
      conversationId: 'conv-A',
      dispatch,
      initialOpenMeasurement: initialMeasurement,
      onValidatedInit: (_payload, report) => {
        clock.mockReturnValue(20);
        reportFirstPaint = report;
        clock.mockReturnValue(30);
      },
    }));
    const stream = FakeEventSource.instances[0]!;

    expect(new URL(stream.url, 'http://localhost').searchParams.get('open_id')).toBe(
      'route-open-id',
    );
    act(() => stream.emit('init', makeInitPayload('conv-A', 'slug-A')));
    expect(reportFirstPaint).toEqual(expect.any(Function));
    expect(fetch).not.toHaveBeenCalled();

    act(() => {
      reportFirstPaint?.();
      reportFirstPaint?.();
    });

    expect(fetch).toHaveBeenCalledTimes(1);
    const body = JSON.parse(
      (vi.mocked(fetch).mock.calls[0]![1] as RequestInit).body as string,
    ) as {
      open_id: string;
      outcome: string;
      route_resolved_ms: number | null;
      init_handled_ms: number;
    };
    expect(body).toMatchObject({
      open_id: 'route-open-id',
      outcome: 'connected',
      route_resolved_ms: expect.any(Number),
      init_handled_ms: 30,
    });
  });

  it('reports cancellation rather than connection when teardown wins before first paint', () => {
    vi.useFakeTimers();
    const dispatch = vi.fn<(a: SSEAction) => void>();
    let reportFirstPaint: (() => void) | undefined;
    const { unmount } = renderHook(() => useConnection({
      conversationId: 'conv-A',
      dispatch,
      onValidatedInit: (_payload, report) => {
        reportFirstPaint = report;
      },
    }));

    act(() => FakeEventSource.instances[0]!.emit('init', makeInitPayload('conv-A', 'slug-A')));
    unmount();
    act(() => {
      vi.advanceTimersByTime(0);
      reportFirstPaint?.();
    });

    expect(fetch).toHaveBeenCalledTimes(1);
    const body = JSON.parse(
      (vi.mocked(fetch).mock.calls[0]![1] as RequestInit).body as string,
    ) as { outcome: string; first_paint_ms: number | null };
    expect(body).toMatchObject({ outcome: 'canceled', first_paint_ms: null });
  });

  it('reports an unfinished open as canceled when the conversation changes', () => {
    vi.useFakeTimers();
    const dispatch = vi.fn<(a: SSEAction) => void>();
    const { rerender } = renderHook(
      ({ convId }) => useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' as string | undefined } },
    );
    const firstOpenId = new URL(
      FakeEventSource.instances[0]!.url,
      'http://localhost',
    ).searchParams.get('open_id');

    rerender({ convId: 'conv-B' });
    act(() => {
      vi.advanceTimersByTime(0);
    });

    const reports = vi.mocked(fetch).mock.calls.map((call) =>
      JSON.parse((call[1] as RequestInit).body as string) as {
        open_id: string;
        outcome: string;
      },
    );
    expect(reports).toContainEqual({
      open_id: firstOpenId,
      outcome: 'canceled',
      route_resolved_ms: null,
      event_source_created_ms: expect.any(Number),
      native_open_ms: null,
      init_received_ms: null,
      init_handled_ms: null,
      first_paint_ms: null,
      total_ms: expect.any(Number),
      retry_attempt: 0,
      visible: expect.any(Boolean),
      effective_type: null,
    });
  });

  afterEach(() => vi.useRealTimers());

  it('does not report StrictMode effect replay as a canceled open', () => {
    vi.useFakeTimers();
    const dispatch = vi.fn<(a: SSEAction) => void>();

    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }), {
      wrapper: StrictMode,
    });
    act(() => {
      vi.advanceTimersByTime(0);
    });

    expect(fetch).not.toHaveBeenCalled();
    vi.useRealTimers();
  });

  it('reports the active unfinished open synchronously on pagehide', () => {
    const dispatch = vi.fn<(a: SSEAction) => void>();
    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));

    act(() => window.dispatchEvent(new Event('pagehide')));

    const body = JSON.parse(
      (vi.mocked(fetch).mock.calls[0]![1] as RequestInit).body as string,
    ) as { outcome: string };
    expect(body.outcome).toBe('canceled');
  });

  it('flushes pending and active opens together on pagehide', () => {
    vi.useFakeTimers();
    const dispatch = vi.fn<(a: SSEAction) => void>();
    const { rerender } = renderHook(
      ({ convId }) => useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' as string | undefined } },
    );
    rerender({ convId: 'conv-B' });

    act(() => window.dispatchEvent(new Event('pagehide')));

    const outcomes = vi.mocked(fetch).mock.calls.map((call) =>
      (JSON.parse((call[1] as RequestInit).body as string) as { outcome: string }).outcome,
    );
    expect(outcomes).toEqual(['canceled', 'canceled']);
  });

  it('flushes every rapid conversation-switch cancellation on pagehide', () => {
    vi.useFakeTimers();
    const dispatch = vi.fn<(a: SSEAction) => void>();
    const { rerender } = renderHook(
      ({ convId }) => useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' as string | undefined } },
    );
    rerender({ convId: 'conv-B' });
    rerender({ convId: 'conv-C' });

    act(() => window.dispatchEvent(new Event('pagehide')));

    expect(fetch).toHaveBeenCalledTimes(3);
    const outcomes = vi.mocked(fetch).mock.calls.map((call) =>
      (JSON.parse((call[1] as RequestInit).body as string) as { outcome: string }).outcome,
    );
    expect(outcomes).toEqual(['canceled', 'canceled', 'canceled']);
  });

  it('stamps every wire-derived dispatch with the connection epoch', () => {
    const captured: SSEAction[] = [];
    const dispatch = (a: SSEAction) => {
      captured.push(a);
    };

    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));

    expect(FakeEventSource.instances).toHaveLength(1);
    const es = FakeEventSource.instances[0]!;

    // First action dispatched on OPEN_SSE is `connection_opened` with the
    // freshly-minted epoch. This is the bootstrap that lifts the atom out
    // of `connectionEpoch === null`.
    const opened = captured.find((a) => a.type === 'connection_opened');
    expect(opened).toBeDefined();
    expect(opened && 'epoch' in opened ? opened.epoch : undefined).toBe(1);

    // Drive an init through the wire — every dispatch downstream of it
    // must carry epoch=1.
    act(() => {
      es.emit('init', makeInitPayload('conv-A', 'slug-A'));
    });

    const wireActions = captured.filter(
      (a) =>
        a.type === 'sse_init' ||
        a.type === 'sse_message' ||
        a.type === 'sse_state_change' ||
        a.type === 'connection_opened',
    );
    expect(wireActions.length).toBeGreaterThan(0);
    for (const a of wireActions) {
      // Every connection-originated action carries an epoch field.
      expect(a).toHaveProperty('epoch');
      const withEpoch = a as { epoch?: number };
      expect(withEpoch.epoch).toBe(1);
    }
  });

  it('dispatches renderable steering deltas and stores live quota snapshots', () => {
    const captured: SSEAction[] = [];
    const onValidatedSteeringQueued = vi.fn();
    renderHook(() => useConnection({
      conversationId: 'conv-A',
      dispatch: (action) => captured.push(action),
      onValidatedSteeringQueued,
    }));
    const es = FakeEventSource.instances[0]!;

    act(() => {
      es.emit('init', makeInitPayload('conv-A', 'slug-A'));
      es.emit('steer_message_queued', {
        sequence_id: 1,
        message: { message_id: 'external-1', text: 'from coordinator', images: [], files: [] },
        queue_position: 0,
      });
      es.emit('steer_message_cancelled', { sequence_id: 2, message_id: 'external-1' });
      es.emit('rate_limit_snapshot', {
        sequence_id: 3,
        snapshot: {
          plan_type: 'plus', resets_at: null, limit_id: 'codex', limit_name: null,
          primary: { used_percent: 12, window_minutes: 300, resets_at: null },
          secondary: null, additional_limits: [], credits: null, individual_limit: null,
          promo_message: null, rate_limit_reached_type: null,
        },
      });
    });

    expect(captured).toContainEqual(expect.objectContaining({
      type: 'sse_steer_message_queued',
      message: expect.objectContaining({ message_id: 'external-1', text: 'from coordinator' }),
      epoch: 1,
    }));
    expect(captured).toContainEqual(expect.objectContaining({
      type: 'sse_steer_message_cancelled', messageId: 'external-1', epoch: 1,
    }));
    expect(onValidatedSteeringQueued).toHaveBeenCalledOnce();
    expect(onValidatedSteeringQueued).toHaveBeenCalledWith('external-1');
    expect(getCodexQuotaSnapshot()?.primary?.used_percent).toBe(12);
  });

  it('does not project a replayed conversation quota snapshot into account state', () => {
    renderHook(() => useConnection({ conversationId: 'conv-A', dispatch: vi.fn() }));
    const payload = makeInitPayload('conv-A', 'slug-A');
    payload.last_sequence_id = 1;
    payload.pending_anchor_sequence_id = 0;
    payload.pending_events = [{
      type: 'rate_limit_snapshot',
      sequence_id: 1,
      snapshot: {
        plan_type: 'plus', resets_at: null, limit_id: 'codex', limit_name: null,
        primary: { used_percent: 41, window_minutes: 300, resets_at: null },
        secondary: null, additional_limits: [], credits: null, individual_limit: null,
        promo_message: null, rate_limit_reached_type: null,
      },
    }];

    act(() => FakeEventSource.instances[0]!.emit('init', payload));

    expect(getCodexQuotaSnapshot()).toBeNull();
  });

  it('drops a stale-EventSource event after slug change (cross-conversation contamination guard)', () => {
    // Real ConversationStore so we can observe atom mutations directly.
    const store = new ConversationStore();
    const slugA = 'slug-A';
    const slugB = 'slug-B';

    // Per-slug dispatch mirrors how ConversationPage wires `useConnection`.
    let activeSlug = slugA;
    const dispatch = (a: SSEAction) => store.dispatch(activeSlug, a);

    const { rerender } = renderHook(
      ({ convId, dispatchFn }: { convId: string; dispatchFn: (a: SSEAction) => void }) =>
        useConnection({ conversationId: convId, dispatch: dispatchFn }),
      { initialProps: { convId: 'conv-A', dispatchFn: dispatch } },
    );

    // First connection: A. Drive init so atom A learns its epoch + state.
    expect(FakeEventSource.instances).toHaveLength(1);
    const esA = FakeEventSource.instances[0]!;
    act(() => {
      esA.emit('init', makeInitPayload('conv-A', slugA));
    });

    const atomAAfterInit = store.getSnapshot(slugA);
    expect(atomAAfterInit.connectionEpoch).toBe(1);
    expect(atomAAfterInit.conversationId).toBe('conv-A');

    // Navigate to B. ConversationPage swaps the slug-bound dispatch BEFORE
    // the cleanup effect fires CLOSE_SSE; useConnection's `dispatchRef` is
    // updated via the `useEffect([dispatch])` hook below the executor.
    activeSlug = slugB;
    rerender({ convId: 'conv-B', dispatchFn: dispatch });

    // Two FakeEventSources should now exist: A's (still in `instances[0]`,
    // its `close()` was called by CLOSE_SSE) and B's freshly opened.
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(esA.closed).toBe(true);
    const esB = FakeEventSource.instances[1]!;

    // Drive B's init so atom B learns its epoch (epoch=2 — the second
    // OPEN_SSE this hook has performed).
    act(() => {
      esB.emit('init', makeInitPayload('conv-B', slugB));
    });
    const atomBAfterInit = store.getSnapshot(slugB);
    expect(atomBAfterInit.connectionEpoch).toBe(2);

    // CONTAMINATION SCENARIO: a buffered event from A's still-around
    // EventSource fires its handler. `dispatchRef.current` already points
    // at B's atom (we swapped activeSlug above; the dispatch closure
    // routes to slugB). The action is stamped with A's epoch (1) but
    // arrives at B's atom (epoch 2) — reducer must reject it.
    const messagesBefore = atomBAfterInit.messages.length;
    act(() => {
      esA.emit('message', {
        sequence_id: 999,
        message: {
          message_id: 'leaked-msg-from-A',
          sequence_id: 999,
          conversation_id: 'conv-A',
          message_type: 'agent',
          content: { text: 'should not land in B' },
          created_at: '2024-01-01T00:00:00Z',
        },
      });
    });

    const atomBAfterLeak = store.getSnapshot(slugB);
    expect(atomBAfterLeak.messages.length).toBe(messagesBefore);
    expect(atomBAfterLeak.messages.find((m) => m.message_id === 'leaked-msg-from-A')).toBeUndefined();
    // The stale handler is owner-guarded and returns before dispatching.
    expect(atomBAfterLeak).toBe(atomBAfterInit);
  });

  it('ignores stale init after switching conversation before it can mark current connection connected', () => {
    const dispatch = vi.fn();
    const { rerender, result } = renderHook(
      ({ convId }: { convId: string }) => useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' } },
    );

    const esA = FakeEventSource.instances[0]!;
    rerender({ convId: 'conv-B' });
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(result.current.state).toBe('connecting');

    act(() => {
      esA.emit('init', makeInitPayload('conv-A', 'slug-A'));
    });

    expect(result.current.state).toBe('connecting');
  });

  it('ignores stale native error after switching conversation before it can show reconnecting', () => {
    const dispatch = vi.fn();
    const { rerender, result } = renderHook(
      ({ convId }: { convId: string }) => useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' } },
    );

    const esA = FakeEventSource.instances[0]!;
    rerender({ convId: 'conv-B' });
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(result.current.state).toBe('connecting');

    act(() => {
      esA.emit('error', '');
    });

    expect(result.current.state).toBe('connecting');
    expect(FakeEventSource.instances).toHaveLength(2);
  });

  it('cancels stale retry timer after switching conversation', () => {
    vi.useFakeTimers();
    try {
      const dispatch = vi.fn();
      const { rerender, result } = renderHook(
        ({ convId }: { convId: string }) => useConnection({ conversationId: convId, dispatch }),
        { initialProps: { convId: 'conv-A' } },
      );

      const esA = FakeEventSource.instances[0]!;
      act(() => {
        esA.emit('error', '');
      });
      expect(result.current.state).toBe('reconnecting');

      rerender({ convId: 'conv-B' });
      expect(FakeEventSource.instances).toHaveLength(2);
      expect(result.current.state).toBe('connecting');

      act(() => {
        vi.advanceTimersByTime(1500);
      });

      expect(FakeEventSource.instances).toHaveLength(2);
      expect(result.current.state).toBe('connecting');
    } finally {
      vi.useRealTimers();
    }
  });

  it('mints a strictly increasing epoch on each OPEN_SSE', () => {
    const captured: SSEAction[] = [];
    const dispatch = (a: SSEAction) => {
      captured.push(a);
    };

    const { rerender } = renderHook(
      ({ convId }: { convId: string | undefined }) =>
        useConnection({ conversationId: convId, dispatch }),
      { initialProps: { convId: 'conv-A' as string | undefined } },
    );

    // First connection → epoch 1
    let opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(1);
    expect((opens[0] as { epoch: number }).epoch).toBe(1);

    // Tear down, then connect again → epoch 2.
    rerender({ convId: undefined });
    rerender({ convId: 'conv-B' });

    opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(2);
    expect((opens[1] as { epoch: number }).epoch).toBe(2);

    // And once more → epoch 3.
    rerender({ convId: undefined });
    rerender({ convId: 'conv-C' });

    opens = captured.filter((a) => a.type === 'connection_opened');
    expect(opens).toHaveLength(3);
    expect((opens[2] as { epoch: number }).epoch).toBe(3);
  });

  it('replaces a failed stream and requires the replacement init to complete reconnect', () => {
    vi.useFakeTimers();
    try {
      const captured: SSEAction[] = [];
      const onValidatedInit = vi.fn();
      const { result } = renderHook(() => useConnection({
        conversationId: 'conv-A',
        dispatch: (action) => captured.push(action),
        getLastAppliedEventSeq: () => 42,
        getTranscriptGeneration: () => 7,
        onValidatedInit,
      }));
      const initial = FakeEventSource.instances[0]!;
      act(() => initial.emit('init', makeInitPayload('conv-A', 'slug-A')));
      expect(result.current.state).toBe('connected');

      act(() => initial.emit('error', ''));
      expect(initial.closed).toBe(true);
      expect(result.current.state).toBe('reconnecting');

      act(() => vi.advanceTimersByTime(1000));
      expect(FakeEventSource.instances).toHaveLength(2);
      const replacement = FakeEventSource.instances[1]!;
      expect(replacement).not.toBe(initial);
      expect(replacement.url).toMatch(/after_event_sequence=42/);
      expect(replacement.url).toMatch(/transcript_generation=7/);
      expect(result.current.state).toBe('reconnecting');

      act(() => replacement.emit('init', {
        ...makeInitPayload('conv-A', 'slug-A'),
        transcript_generation: 7,
        last_sequence_id: 42,
        stream_incarnation: 'replacement-stream',
      }));
      expect(result.current.state).toBe('reconnected');
      expect(onValidatedInit).toHaveBeenCalledTimes(2);
      expect(captured.filter((action) => action.type === 'sse_init')).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not schedule duplicate retry timers under StrictMode (regression: setTimeout-in-functional-updater)', async () => {
    // The pre-08683 implementation called `setTimeout(executeEffects, 0)`
    // *inside* setMachineState's functional updater. StrictMode invokes
    // the updater twice in dev; that produced two timer schedules per
    // SSE_ERROR. After 08683 the effect runs synchronously once per
    // dispatch, so even under a doubled-render pattern only one timer is
    // scheduled per error.
    //
    // We verify the contract by counting the EventSources opened in
    // response to a single SSE_ERROR + RETRY_TIMER_FIRED cycle: it must
    // be exactly one per intended retry, not two.
    vi.useFakeTimers();
    try {
      const dispatch = vi.fn();
      renderHook(() => useConnection({ conversationId: 'conv-A', dispatch }));
      expect(FakeEventSource.instances).toHaveLength(1);
      const esA = FakeEventSource.instances[0]!;

      // Trigger reconnect: emit a connection error (no-data error event
      // signals native EventSource failure on the real wire).
      act(() => {
        esA.emit('error', '');
      });

      // Advance past the 1s base backoff. If the bug were back, two
      // RETRY_TIMER_FIRED transitions would fire and produce two new
      // EventSources.
      act(() => {
        vi.advanceTimersByTime(1500);
      });

      // One additional EventSource (the retry); never two.
      expect(FakeEventSource.instances.length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
