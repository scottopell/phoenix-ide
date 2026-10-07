import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProductConversationListRow } from '../api';
import { ApiResponseError, api } from '../api';
import { ConversationListPage } from './ConversationListPage';

const pageMocks = vi.hoisted(() => ({
  refresh: vi.fn().mockResolvedValue(undefined),
  isOnline: true,
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      archiveChain: vi.fn(),
      archiveConversation: vi.fn(),
      closeProductConversation: vi.fn(),
      codexLoginPreflight: vi.fn(),
      listProductConversations: vi.fn(),
      renameProductConversation: vi.fn(),
      deleteChain: vi.fn(),
      deleteConversation: vi.fn(),
    },
  };
});

vi.mock('../hooks', async () => {
  const actual = await vi.importActual<typeof import('../hooks')>('../hooks');
  return {
    ...actual,
    useAutoAuth: () => ({ showAuthPanel: false, setShowAuthPanel: vi.fn() }),
    useIsDesktop: () => false,
    useModels: () => ({ credentialStatus: 'valid' }),
    useTheme: () => ({ theme: 'dark', toggleTheme: vi.fn() }),
  };
});

vi.mock('../hooks/useAppMachine', () => ({
  useAppMachine: () => ({
    isOnline: pageMocks.isOnline,
    isReady: true,
    initError: null,
    pendingOpsCount: 0,
    queueOperation: vi.fn(),
  }),
}));

vi.mock('../hooks/useToast', () => ({
  useToast: () => ({
    toasts: [],
    dismissToast: vi.fn(),
    showWarning: vi.fn(),
    showError: vi.fn(),
  }),
}));

vi.mock('../conversation', () => ({
  useConversationsList: () => ({ active: [], archived: [] }),
  useConversationsRefresh: () => ({ refresh: pageMocks.refresh }),
}));

vi.mock('../modelsPoller', () => ({
  refreshModels: vi.fn(),
  subscribeModels: () => () => {},
}));

const productConversation = (title = 'Mobile Product'): ProductConversationListRow => ({
  product_conversation_id: 'pc-mobile',
  canonical_route: '/product-conversations/pc-mobile',
  canonical_root: { transcript_row_id: 'root-mobile', slug: 'mobile-product', title },
  lifecycle: { state: 'open', close_action: { availability: 'available' } },
  latest_transcript_row_id: 'latest-mobile',
  updated_at: '2026-01-01T00:00:00Z',
  presentation: { kind: 'state', display_name: title, presentation_mode: 'idle' },
});

function touchActivate(element: HTMLElement): void {
  fireEvent.pointerDown(element, { pointerId: 1, pointerType: 'touch', isPrimary: true });
  fireEvent.pointerUp(element, { pointerId: 1, pointerType: 'touch', isPrimary: true });
  fireEvent.click(element, { detail: 1 });
}

describe('ConversationListPage mobile ProductConversation actions', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    pageMocks.refresh.mockResolvedValue(undefined);
    pageMocks.isOnline = true;
    vi.mocked(api.codexLoginPreflight).mockResolvedValue({} as never);
    vi.mocked(api.listProductConversations).mockResolvedValue({
      product_conversations: [productConversation()],
    });
  });

  it('renames through the production mobile list touch target', async () => {
    const renamed = productConversation('Renamed on Mobile');
    vi.mocked(api.renameProductConversation).mockResolvedValue(renamed);
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' }));
    touchActivate(await screen.findByRole('button', { name: 'Rename conversation Mobile Product' }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Renamed on Mobile' } });
    touchActivate(screen.getByRole('button', { name: 'Rename' }));

    await waitFor(() => expect(api.renameProductConversation).toHaveBeenCalledWith(
      'pc-mobile',
      'Renamed on Mobile',
    ));
    expect(await screen.findByText('Renamed on Mobile')).toBeInTheDocument();
  });

  it('does not offer ProductConversation Close while the mobile page is offline', async () => {
    pageMocks.isOnline = false;
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    expect(await screen.findByText('Mobile Product')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Close conversation Mobile Product' })).toBeNull();
  });

  it.each(['open', 'history'] as const)('restores Actions focus after %s confirmation cancellation', async (state) => {
    if (state === 'history') vi.mocked(api.listProductConversations).mockResolvedValue({ product_conversations: [{ ...productConversation(), lifecycle: { state: 'history' } }] });
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);
    if (state === 'history') fireEvent.click(await screen.findByRole('button', { name: 'History 1' }));
    const trigger = await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' });
    trigger.focus();
    fireEvent.click(trigger);
    const action = screen.getByRole('button', { name: `${state === 'history' ? 'Delete' : 'Close'} conversation Mobile Product` });
    action.focus();
    fireEvent.click(action);
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveFocus();
    fireEvent.click(cancel);
    expect(trigger).toHaveFocus();
    expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();
    expect(api.closeProductConversation).not.toHaveBeenCalled();
    expect(api.deleteChain).not.toHaveBeenCalled();
  });

  it('uses aggregate deletion for History with one parent transcript', async () => {
    const history = {
      ...productConversation(),
      lifecycle: { state: 'history' as const },
      latest_transcript_row_id: 'root-mobile',
    };
    vi.mocked(api.listProductConversations).mockResolvedValue({ product_conversations: [history] });
    vi.mocked(api.deleteChain).mockResolvedValue({
      success: true,
      outcome: {
        type: 'deleted',
        deleted_conversation_ids: ['root-mobile', 'agent-mobile'],
      },
    });
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'History 1' }));
    touchActivate(await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' }));
    touchActivate(await screen.findByRole('button', { name: 'Delete conversation Mobile Product' }));
    touchActivate(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(api.deleteChain).toHaveBeenCalledWith('root-mobile'));
    expect(api.deleteConversation).not.toHaveBeenCalled();
  });

  it('treats an authoritative 404 while deleting History as completed', async () => {
    const history = {
      ...productConversation(),
      lifecycle: { state: 'history' as const },
      latest_transcript_row_id: 'successor-mobile',
    };
    vi.mocked(api.listProductConversations).mockResolvedValue({ product_conversations: [history] });
    vi.mocked(api.deleteChain).mockRejectedValue(new ApiResponseError('gone', 404));
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'History 1' }));
    touchActivate(await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' }));
    touchActivate(await screen.findByRole('button', { name: 'Delete conversation Mobile Product' }));
    touchActivate(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(api.deleteChain).toHaveBeenCalledWith('root-mobile'));
    await waitFor(() => expect(screen.queryByText(/gone/)).toBeNull());
    expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull();
  });

  it('removes deleted History locally when the invalidation refresh fails', async () => {
    const history = {
      ...productConversation(),
      lifecycle: { state: 'history' as const },
      latest_transcript_row_id: 'successor-mobile',
    };
    vi.mocked(api.listProductConversations)
      .mockResolvedValueOnce({ product_conversations: [history] })
      .mockRejectedValueOnce(new Error('refresh failed'));
    vi.mocked(api.deleteChain).mockResolvedValue({
      success: true,
      outcome: {
        type: 'deleted',
        deleted_conversation_ids: ['root-mobile', 'agent-mobile'],
      },
    });
    const hardDeleted = vi.fn();
    window.addEventListener('phoenix:conversation-hard-deleted', hardDeleted, { once: true });
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'History 1' }));
    touchActivate(await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' }));
    touchActivate(await screen.findByRole('button', { name: 'Delete conversation Mobile Product' }));
    touchActivate(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(api.deleteChain).toHaveBeenCalledWith('root-mobile'));
    await waitFor(() => expect(screen.queryByText('Mobile Product')).toBeNull());
    expect(hardDeleted).toHaveBeenCalledWith(expect.objectContaining({
      detail: {
        conversationId: 'pc-mobile',
        deletedConversationIds: ['root-mobile', 'agent-mobile'],
      },
    }));
  });

  it('closes through the production mobile list touch target and aggregate confirmation', async () => {
    vi.mocked(api.closeProductConversation).mockResolvedValue(undefined);
    vi.mocked(api.listProductConversations).mockResolvedValue({
      product_conversations: [{ ...productConversation(), latest_transcript_row_id: 'root-mobile' }],
    });
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'Actions for conversation Mobile Product' }));
    touchActivate(await screen.findByRole('button', { name: 'Close conversation Mobile Product' }));
    expect(screen.getByText(/moves the conversation to read-only History and stops its active work/)).toBeInTheDocument();
    touchActivate(screen.getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(api.closeProductConversation).toHaveBeenCalledWith('pc-mobile'));
  });
});
