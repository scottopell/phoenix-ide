// SSE-driven sidebar update integration test (task 08684).
//
// The cardinal acceptance criterion: when the conversation atom for slug X
// receives an `sse_conversation_update` (e.g. a `cwd` flip after task
// approval, or an `updated_at` bump from the server), the sidebar row for
// X must reflect that change within one render frame — NOT after the next
// 5s poll. Pre-08684 the sidebar read from a separate polled
// `Conversation[]` so this lag was structural; post-08684 the store is the
// single source of truth and `useConversationsList` derives directly from
// the same atom that SSE writes into.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, act, waitFor } from '@testing-library/react';
import {
  ConversationProvider,
  ConversationStore,
  useConversationsList,
} from './';
import { ConversationContext } from './ConversationContext';
import type { Conversation } from '../api';
import { useContext } from 'react';

// The polling refresh tries to call api.listConversations on mount. We
// don't want the test to hit the network or rely on its results — we
// want to verify the in-store SSE-driven update path. Mock the api
// surface to no-op promises so the refresh service is a no-op.
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

// Cache also a no-op — the refresh service awaits cacheDB.init() which is
// fine in tests because the test-setup provides a stub IndexedDB. We mock
// it explicitly so the test doesn't depend on the DB driver.
vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn(() => Promise.resolve([])),
    syncConversations: vi.fn(() => Promise.resolve()),
    putConversation: vi.fn(() => Promise.resolve()),
  },
}));

function makeConv(slug: string, overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: `conv-${slug}`,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/repo',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-06-01T00:00:00Z',
    message_count: 0,
    archived: false,
    ...overrides,
  } as Conversation;
}

/**
 * Tiny consumer that renders the active list and exposes the store via
 * a callback ref so the test can drive SSE-style dispatches into it.
 * Mirrors how Sidebar consumes the hook.
 */
function Consumer({ onStore }: { onStore: (store: ConversationStore) => void }) {
  const store = useContext(ConversationContext);
  if (store) onStore(store);
  const { active } = useConversationsList();
  return (
    <ul>
      {active.map((c) => (
        <li key={c.slug} data-testid={`row-${c.slug}`}>
          <span data-testid={`cwd-${c.slug}`}>{c.cwd}</span>
          <span data-testid={`updated-${c.slug}`}>{c.updated_at}</span>
        </li>
      ))}
    </ul>
  );
}

describe('useConversationsList SSE → sidebar reactivity (task 08684)', () => {
  it('SSE-driven update on non-active conversation reflects in the list immediately', async () => {
    let store: ConversationStore | undefined;
    const captureStore = (s: ConversationStore) => {
      store = s;
    };

    render(
      <ConversationProvider>
        <Consumer onStore={captureStore} />
      </ConversationProvider>,
    );
    expect(store).toBeDefined();

    // Seed two conversations into the store as if a refresh had landed.
    act(() => {
      store!.upsertSnapshot('alpha', makeConv('alpha', { cwd: '/repo/main' }));
      store!.upsertSnapshot('beta', makeConv('beta', { cwd: '/repo/main' }));
    });

    await waitFor(() => {
      expect(screen.getByTestId('cwd-alpha').textContent).toBe('/repo/main');
      expect(screen.getByTestId('cwd-beta').textContent).toBe('/repo/main');
    });

    // SSE: sse_conversation_update arrives for slug 'beta' — the user is
    // currently viewing 'alpha', so 'beta' is not the active slug. Pre-
    // 08684, this update would land in beta's atom but the sidebar
    // (reading from DesktopLayout's separate polled array) would not
    // reflect it until the next 5s tick. Now: the same atom IS the row,
    // so the list re-derives on dispatch.
    act(() => {
      // First lift the connection epoch so subsequent stamped events
      // pass the isStaleEpoch guard.
      store!.dispatch('beta', { type: 'connection_opened', epoch: 1 });
      store!.dispatch('beta', {
        type: 'sse_conversation_update',
        epoch: 1,
        sequenceId: 5,
        updates: { cwd: '/repo/feature-x', updated_at: '2024-06-02T00:00:00Z' },
      });
    });

    // Within one render tick, the sidebar reflects the new cwd.
    await waitFor(() => {
      expect(screen.getByTestId('cwd-beta').textContent).toBe('/repo/feature-x');
    });
    // Alpha untouched.
    expect(screen.getByTestId('cwd-alpha').textContent).toBe('/repo/main');
  });

  it('store-level upsert with stale updated_at is dropped — cache-clobber regression', async () => {
    // Polling tick that returns rows older than what SSE has already
    // pushed must not regress the conversation row.
    let store: ConversationStore | undefined;
    const captureStore = (s: ConversationStore) => {
      store = s;
    };

    render(
      <ConversationProvider>
        <Consumer onStore={captureStore} />
      </ConversationProvider>,
    );

    act(() => {
      // Initial: SSE-driven row at updated_at=2024-06-02.
      store!.upsertSnapshot(
        'alpha',
        makeConv('alpha', { cwd: '/fresh', updated_at: '2024-06-02T00:00:00Z' }),
      );
    });
    await waitFor(() => {
      expect(screen.getByTestId('cwd-alpha').textContent).toBe('/fresh');
    });

    act(() => {
      // Polling tick arrives later but with an older snapshot.
      // The upsertSnapshot guard refuses to regress.
      store!.upsertSnapshot(
        'alpha',
        makeConv('alpha', { cwd: '/stale', updated_at: '2024-06-01T00:00:00Z' }),
      );
    });

    // List unchanged; no flicker to /stale.
    expect(screen.getByTestId('cwd-alpha').textContent).toBe('/fresh');
  });
});
