// Structural perf-isolation guarantee for the ConversationPage subscription
// (Finding B). The page renders from `useConversationView`, which excludes the
// two highest-frequency atom fields:
//
//   - `streamingBuffer` — churns on every `sse_token`
//   - `lastSseEventAt`   — churns on every observed event (token + `ping`)
//
// so neither a streaming token nor a heartbeat bump may re-render the page
// body. The watchdog clock is consumed separately via `useLastSseEventAt`,
// which MUST re-render its own (StateBar-level) subscriber on the bump.
//
// If a future change reads a volatile field back into the page view, or drops
// the cached-reference contract, the render-count assertions below fail.

import { describe, it, expect, vi } from 'vitest';
import { render, act } from '@testing-library/react';
import { useContext, useRef } from 'react';
import {
  ConversationProvider,
  ConversationStore,
  useConversationView,
  useLastSseEventAt,
  useLastSseEventAtRef,
} from './';
import { ConversationContext } from './ConversationContext';
import type { Conversation } from '../api';

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      listConversations: vi.fn(() => Promise.resolve([])),
      listArchivedConversations: vi.fn(() => Promise.resolve([])),
    },
  };
});

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn(() => Promise.resolve([])),
    syncConversations: vi.fn(() => Promise.resolve()),
    putConversation: vi.fn(() => Promise.resolve()),
  },
}));

const SLUG = 'alpha';
const CONV_ID = 'conv-alpha';

function makeConv(): Conversation {
  return {
    id: CONV_ID,
    slug: SLUG,
    model: 'claude-3-5-sonnet',
    cwd: '/repo',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-06-01T00:00:00Z',
    message_count: 0,
    archived: false,
  } as Conversation;
}

function Harness({
  onStore,
  viewRenders,
  clockRenders,
}: {
  onStore: (s: ConversationStore) => void;
  viewRenders: { current: number };
  clockRenders: { current: number };
}) {
  const store = useContext(ConversationContext);
  if (store) onStore(store);
  return (
    <>
      <PageViewConsumer renders={viewRenders} />
      <ClockConsumer renders={clockRenders} />
    </>
  );
}

function PageViewConsumer({ renders }: { renders: { current: number } }) {
  const [atom] = useConversationView(SLUG);
  renders.current += 1;
  return <div data-testid="phase">{atom.phase.type}</div>;
}

function ClockConsumer({ renders }: { renders: { current: number } }) {
  const lastSseEventAt = useLastSseEventAt(SLUG);
  renders.current += 1;
  const seen = useRef(lastSseEventAt);
  seen.current = lastSseEventAt;
  return <div data-testid="clock">{lastSseEventAt}</div>;
}

describe('useConversationView perf isolation (Finding B)', () => {
  it('does not re-render the page view on sse_token or heartbeat bumps', () => {
    let store: ConversationStore | undefined;
    const viewRenders = { current: 0 };
    const clockRenders = { current: 0 };

    // Drive `lastSseEventAt` off a controlled clock so the heartbeat bump is
    // guaranteed to produce a strictly-greater timestamp than the seeded
    // value. Real `Date.now()` can return the same millisecond, in which case
    // the primitive snapshot is unchanged and the clock subscriber correctly
    // does not re-render — which would flake the assertion below.
    let now = 1_700_000_000_000;
    const nowSpy = vi.spyOn(Date, 'now').mockImplementation(() => now);

    render(
      <ConversationProvider>
        <Harness
          onStore={(s) => (store = s)}
          viewRenders={viewRenders}
          clockRenders={clockRenders}
        />
      </ConversationProvider>,
    );
    expect(store).toBeDefined();

    // Seed a live atom in the llm_requesting phase so sse_token accumulates.
    act(() => {
      store!.dispatch(SLUG, {
        type: 'set_initial_data',
        conversationId: CONV_ID,
        conversation: makeConv(),
        messages: [],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 1,
      });
      store!.dispatch(SLUG, { type: 'connection_opened', epoch: 1 });
      store!.dispatch(SLUG, {
        type: 'local_phase_change',
        phase: { type: 'llm_requesting', attempt: 1 },
        expectedConversationId: CONV_ID,
      });
    });

    const viewBaseline = viewRenders.current;
    const clockBaseline = clockRenders.current;

    // A heartbeat bump (every token + every ping dispatches this). Must NOT
    // touch the page view; MUST re-render the clock subscriber. Advance the
    // controlled clock first so the bumped timestamp is strictly greater.
    now += 1;
    act(() => {
      store!.dispatch(SLUG, { type: 'sse_event_observed' });
    });
    expect(viewRenders.current).toBe(viewBaseline);
    expect(clockRenders.current).toBeGreaterThan(clockBaseline);

    // A streaming token. Changes only streamingBuffer — neither subscriber
    // that excludes it should re-render the page view.
    const viewAfterClock = viewRenders.current;
    act(() => {
      store!.dispatch(SLUG, {
        type: 'sse_token',
        epoch: 1,
        sequenceId: 1,
        delta: 'hello ',
        requestId: 'req-1',
      });
      store!.dispatch(SLUG, {
        type: 'sse_token',
        epoch: 1,
        sequenceId: 2,
        delta: 'world',
        requestId: 'req-1',
      });
    });
    expect(viewRenders.current).toBe(viewAfterClock);

    // Sanity: a real page-relevant change (a new message) DOES re-render the
    // view — the isolation is selective, not a dead subscription.
    act(() => {
      store!.dispatch(SLUG, {
        type: 'sse_message',
        epoch: 1,
        sequenceId: 3,
        message: {
          message_id: 'm1',
          message_type: 'assistant',
          content: [{ type: 'text', text: 'hi' }],
          created_at: '2024-06-01T00:00:01Z',
        } as unknown as import('../api').Message,
      });
    });
    expect(viewRenders.current).toBeGreaterThan(viewAfterClock);

    nowSpy.mockRestore();
  });
});

// The StateBar watchdog samples the heartbeat clock via useLastSseEventAtRef
// (a ref) instead of useLastSseEventAt (a value), so the per-event bump does
// not re-render the StateBar subtree. This pins that no-re-render guarantee:
// if the hook ever reverts to a value subscription, the host render count
// climbs on the heartbeat and this fails.
describe('useLastSseEventAtRef (heartbeat clock — ref, no host re-render)', () => {
  it('tracks the bump in its ref without re-rendering the host', () => {
    let store: ConversationStore | undefined;
    const hostRenders = { current: 0 };
    let clockRef: { current: number } | undefined;

    let now = 1_700_000_000_000;
    const nowSpy = vi.spyOn(Date, 'now').mockImplementation(() => now);

    function RefHost() {
      clockRef = useLastSseEventAtRef(SLUG);
      hostRenders.current += 1;
      return null;
    }
    function Capture() {
      const s = useContext(ConversationContext);
      if (s) store = s;
      return <RefHost />;
    }

    render(
      <ConversationProvider>
        <Capture />
      </ConversationProvider>,
    );

    act(() => {
      store!.dispatch(SLUG, {
        type: 'set_initial_data',
        conversationId: CONV_ID,
        conversation: makeConv(),
        messages: [],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 1,
      });
      store!.dispatch(SLUG, { type: 'connection_opened', epoch: 1 });
    });

    const baselineRenders = hostRenders.current;
    const baselineClock = clockRef!.current;

    // A heartbeat bump with an advanced clock.
    now += 5_000;
    act(() => {
      store!.dispatch(SLUG, { type: 'sse_event_observed' });
    });

    // The ref tracked the new timestamp...
    expect(clockRef!.current).toBe(now);
    expect(clockRef!.current).toBeGreaterThan(baselineClock);
    // ...but the host did NOT re-render on the bump.
    expect(hostRenders.current).toBe(baselineRenders);

    nowSpy.mockRestore();
  });
});
