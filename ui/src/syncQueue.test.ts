import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { PendingOperation } from './cache';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    archiveConversation: vi.fn(),
    archiveChain: vi.fn(),
    deleteConversation: vi.fn(),
  },
}));

vi.mock('./api', async () => {
  const actual = await vi.importActual<typeof import('./api')>('./api');
  return { ...actual, api: apiMock };
});

import { syncQueue } from './syncQueue';

function makeOp(type: string): PendingOperation {
  return {
    id: 'op-1',
    type,
    conversationId: 'conv-1',
    payload: {},
    createdAt: new Date(),
    retryCount: 0,
    status: 'pending',
  } as unknown as PendingOperation;
}

describe('SyncQueue legacy op draining', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  // Archive became a terminal lifecycle transition and the unarchive endpoints
  // were removed, but an offline `unarchive` op may already sit in IndexedDB
  // from before the upgrade. It must drain as a no-op (resolve, hit no API) so
  // the row is deleted and the offline banner clears — not throw forever.
  it('drains a legacy unarchive op without throwing or calling any API', async () => {
    await expect(syncQueue.processOperation(makeOp('unarchive'))).resolves.toBeUndefined();
    expect(apiMock.archiveConversation).not.toHaveBeenCalled();
    expect(apiMock.deleteConversation).not.toHaveBeenCalled();
  });

  it('drains a legacy unarchive_chain op without throwing or calling any API', async () => {
    await expect(syncQueue.processOperation(makeOp('unarchive_chain'))).resolves.toBeUndefined();
    expect(apiMock.archiveChain).not.toHaveBeenCalled();
  });

  it('still throws on a genuinely unknown op type so the row is retried, not silently dropped', async () => {
    await expect(syncQueue.processOperation(makeOp('teleport'))).rejects.toThrow(
      /Unknown operation type/,
    );
  });

  it('processes a current archive op through the api', async () => {
    await syncQueue.processOperation(makeOp('archive'));
    expect(apiMock.archiveConversation).toHaveBeenCalledWith('conv-1');
  });
});
