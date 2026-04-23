// SharePage SSE schema validation smoke tests (task 02678).
//
// The four parse sites in SharePage (init, message, state_change, token) now
// route through `parseEvent` from `hooks/useConnection`. `sseSchemas.test.ts`
// exercises the schema-level rejection behaviour for every event type. These
// tests cover the SharePage-specific piece: the local dispatch adapter that
// translates `sse_error` actions into SharePage's own error UI state.
//
// What we verify here (and don't duplicate from sseSchemas.test.ts):
//   - a malformed `init` payload never reaches `handleSseInit` — instead the
//     share view lands in its 'error' state with a user-visible banner.
//   - a malformed `message` payload on an already-connected stream also lands
//     on the error banner (proving the adapter runs for every event type,
//     not just init).
//
// `import.meta.env.DEV` is true under vitest by default; the parseEvent helper
// throws in dev mode to make contract drift loud. Each failure-path test flips
// DEV to false so the prod-mode dispatch branch runs — which is what
// SharePage's adapter consumes.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import { SharePage } from './SharePage';

type Listener = (event: MessageEvent) => void;

/** Minimal EventSource stand-in: captures listeners by event type so the
 *  test can fire synthetic events on demand. */
class MockEventSource {
  static instances: MockEventSource[] = [];
  listeners = new Map<string, Listener[]>();
  readyState = 0;
  onerror: ((e: Event) => void) | null = null;
  url: string;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: Listener): void {
    const arr = this.listeners.get(type) ?? [];
    arr.push(listener);
    this.listeners.set(type, arr);
  }

  removeEventListener(): void {
    // not exercised
  }

  close(): void {
    this.readyState = 2; // CLOSED
  }

  /** Fire a synthetic event with a pre-stringified `data` payload. */
  emit(type: string, data: unknown): void {
    const payload = typeof data === 'string' ? data : JSON.stringify(data);
    const evt = { data: payload } as unknown as MessageEvent;
    const arr = this.listeners.get(type) ?? [];
    for (const l of arr) l(evt);
  }
}

function inProdMode<T>(fn: () => T): T {
  const env = import.meta.env as unknown as Record<string, unknown>;
  const original = env['DEV'];
  env['DEV'] = false;
  try {
    return fn();
  } finally {
    env['DEV'] = original;
  }
}

function renderSharePage() {
  return render(
    <MemoryRouter initialEntries={['/share/tok-1']}>
      <Routes>
        <Route path="/share/:token" element={<SharePage />} />
      </Routes>
    </MemoryRouter>
  );
}

describe('SharePage SSE schema validation', () => {
  const originalEventSource = globalThis.EventSource;

  beforeEach(() => {
    MockEventSource.instances = [];
    // Spy-free install — vitest's vi.stubGlobal handles cleanup via vi.unstubAllGlobals,
    // but we want explicit teardown below to restore the real polyfill.
    (globalThis as unknown as { EventSource: unknown }).EventSource =
      MockEventSource as unknown as typeof EventSource;
    // Silence expected console.error output from parseEvent's dev-mode path
    // in case a test forgets to wrap in inProdMode.
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    (globalThis as unknown as { EventSource: unknown }).EventSource =
      originalEventSource;
    vi.restoreAllMocks();
  });

  it('shows an error banner when init payload is malformed (missing required fields)', () => {
    renderSharePage();

    // The effect runs after mount; the mock EventSource is constructed then.
    expect(MockEventSource.instances).toHaveLength(1);
    const es = MockEventSource.instances[0]!;

    inProdMode(() => {
      act(() => {
        // Malformed init: missing conversation, messages, last_sequence_id, etc.
        es.emit('init', { sequence_id: 0 });
      });
    });

    // The share view should land on the error state — malformed init never
    // reaches handleSseInit, so status === 'error' (not 'connected'). The
    // adapter maps the schema-violation BackendError into the banner text.
    const banner = document.querySelector('.share-banner--error');
    expect(banner).not.toBeNull();
    expect(banner?.textContent ?? '').toMatch(/schema|init/i);
  });

  it('shows an error banner when init payload is not JSON', () => {
    renderSharePage();
    const es = MockEventSource.instances[0]!;

    inProdMode(() => {
      act(() => {
        es.emit('init', '{not: json');
      });
    });

    expect(screen.getByText('Failed to parse server data')).toBeInTheDocument();
  });

  it('surfaces schema violations on post-init events via the same error banner', () => {
    renderSharePage();
    const es = MockEventSource.instances[0]!;

    // First, deliver a valid init so we reach the 'connected' state.
    act(() => {
      es.emit('init', {
        sequence_id: 0,
        conversation: { id: 'conv-1', slug: 'test', model: 'test' },
        messages: [],
        agent_working: false,
        last_sequence_id: 0,
        display_state: 'idle',
        context_window_size: 0,
        model_context_window: 200_000,
        breadcrumbs: [],
        commits_behind: 0,
        commits_ahead: 0,
        project_name: null,
      });
    });

    // Now send a malformed message — the adapter should flip status to 'error'
    // rather than corrupting the message list.
    inProdMode(() => {
      act(() => {
        es.emit('message', { sequence_id: 'not-a-number', message: {} });
      });
    });

    // The error banner replaces the connected view. We look for the banner's
    // text — either the schema violation message (BackendError) or the generic
    // parse-error copy.
    const banner = document.querySelector('.share-banner--error');
    expect(banner).not.toBeNull();
    expect(banner?.textContent ?? '').toMatch(
      /schema|parse|failed/i,
    );
  });
});
