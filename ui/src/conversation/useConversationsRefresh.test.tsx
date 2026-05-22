// Tests for useConversationsRefresh side-effect surface.
//
// REQ-VS-014: the hard-delete cascade must clear the per-conversation
// last-viewer storage entry. Without this, slug-keyed entries accumulate
// for conversations the user has explicitly removed; if the server ever
// recycles a slug, an orphan entry would silently restore a viewer under
// the wrong conversation.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, act, waitFor } from '@testing-library/react';
import { useContext } from 'react';
import {
  ConversationProvider,
  ConversationStore,
} from './';
import { ConversationContext } from './ConversationContext';
import { DraftContext } from './DraftContext';
import type { DraftStore } from './DraftStore';
import {
  getLastViewer,
  setLastViewer,
} from '../storage/lastViewerStorage';
import type { Conversation } from '../api';

// The polling refresh tries to call api.listConversations on mount.
// No-op so the test isolates the hard-delete listener.
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

function makeConv(slug: string, id: string): Conversation {
  return {
    id,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/repo',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-06-01T00:00:00Z',
    message_count: 0,
    archived: false,
  } as Conversation;
}

function CaptureStore({
  onStore,
}: {
  onStore: (s: ConversationStore) => void;
}) {
  const store = useContext(ConversationContext);
  if (store) onStore(store);
  return null;
}

describe('useConversationsRefreshDriver — REQ-VS-014 hard-delete cascade', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('clears the slug-keyed last-viewer entry on phoenix:conversation-hard-deleted', async () => {
    let store: ConversationStore | undefined;
    const captureStore = (s: ConversationStore) => {
      store = s;
    };

    render(
      <ConversationProvider>
        <CaptureStore onStore={captureStore} />
      </ConversationProvider>,
    );
    expect(store).toBeDefined();

    act(() => {
      store!.upsertSnapshot('doomed', makeConv('doomed', 'conv-doomed'));
    });
    setLastViewer('doomed', 'file=%2Frepo%2Ffoo&root=%2Frepo');
    expect(getLastViewer('doomed')).not.toBeNull();

    act(() => {
      window.dispatchEvent(
        new CustomEvent('phoenix:conversation-hard-deleted', {
          detail: { conversationId: 'conv-doomed' },
        }),
      );
    });

    await waitFor(() => {
      expect(getLastViewer('doomed')).toBeNull();
    });
  });

  it('clears the slug-keyed DraftStore atom and the conversationId-keyed localStorage draft', async () => {
    // PR #92 review: when a conversation is hard-deleted, the per-slug
    // DraftStore atom and the localStorage entry keyed by conversationId
    // both have to go — otherwise a slug the server later recycles for
    // a new conversation surfaces stale text in the composer (or worse,
    // re-persists it under the new id once the user types).
    let store: ConversationStore | undefined;
    let draftStore: DraftStore | undefined;
    function CaptureBoth() {
      store = useContext(ConversationContext) ?? undefined;
      draftStore = useContext(DraftContext) ?? undefined;
      return null;
    }

    render(
      <ConversationProvider>
        <CaptureBoth />
      </ConversationProvider>,
    );
    expect(store).toBeDefined();
    expect(draftStore).toBeDefined();

    act(() => {
      store!.upsertSnapshot('doomed', makeConv('doomed', 'conv-doomed'));
      draftStore!.dispatch('doomed', { type: 'set_draft', text: 'half-typed' });
    });
    localStorage.setItem('phoenix:draft:conv-doomed', 'half-typed');
    expect(draftStore!.getSnapshot('doomed').draft).toBe('half-typed');
    expect(localStorage.getItem('phoenix:draft:conv-doomed')).toBe('half-typed');

    act(() => {
      window.dispatchEvent(
        new CustomEvent('phoenix:conversation-hard-deleted', {
          detail: { conversationId: 'conv-doomed' },
        }),
      );
    });

    await waitFor(() => {
      // After remove, getSnapshot lazy-creates a fresh initial atom —
      // the assertion is that the previous draft text is gone.
      expect(draftStore!.getSnapshot('doomed').draft).toBe('');
      expect(localStorage.getItem('phoenix:draft:conv-doomed')).toBeNull();
    });
  });

  it('does not throw when the deleted id is unknown to the store', () => {
    render(
      <ConversationProvider>
        <CaptureStore onStore={() => {}} />
      </ConversationProvider>,
    );

    setLastViewer('orphan', 'file=%2Frepo%2Ffoo&root=%2Frepo');
    expect(() => {
      act(() => {
        window.dispatchEvent(
          new CustomEvent('phoenix:conversation-hard-deleted', {
            detail: { conversationId: 'never-existed' },
          }),
        );
      });
    }).not.toThrow();
    // Storage entry untouched — store has no slug for the unknown id, so
    // there is nothing to clear and the helper isn't invoked.
    expect(getLastViewer('orphan')).not.toBeNull();
  });
});
