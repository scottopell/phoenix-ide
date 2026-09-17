import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProductConversationListRow } from '../api';
import { api } from '../api';
import { ConversationListPage } from './ConversationListPage';

const pageMocks = vi.hoisted(() => ({
  refresh: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      archiveChain: vi.fn(),
      codexLoginPreflight: vi.fn(),
      listProductConversations: vi.fn(),
      renameProductConversation: vi.fn(),
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
    isOnline: true,
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
  ordinary_lifecycle: 'open',
  close_action: { availability: 'available' },
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
    vi.mocked(api.codexLoginPreflight).mockResolvedValue({} as never);
    vi.mocked(api.listProductConversations).mockResolvedValue({
      product_conversations: [productConversation()],
    });
  });

  it('renames through the production mobile list touch target', async () => {
    const renamed = productConversation('Renamed on Mobile');
    vi.mocked(api.renameProductConversation).mockResolvedValue(renamed);
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'Rename product conversation Mobile Product' }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Renamed on Mobile' } });
    touchActivate(screen.getByRole('button', { name: 'Rename' }));

    await waitFor(() => expect(api.renameProductConversation).toHaveBeenCalledWith(
      'pc-mobile',
      'Renamed on Mobile',
    ));
    expect(await screen.findByText('Renamed on Mobile')).toBeInTheDocument();
  });

  it('closes through the production mobile list touch target and aggregate confirmation', async () => {
    vi.mocked(api.archiveChain).mockResolvedValue({} as never);
    render(<MemoryRouter><ConversationListPage /></MemoryRouter>);

    touchActivate(await screen.findByRole('button', { name: 'Close product conversation Mobile Product' }));
    expect(screen.getByText(/move the entire product conversation to read-only History and stop its active work/)).toBeInTheDocument();
    touchActivate(screen.getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(api.archiveChain).toHaveBeenCalledWith('root-mobile'));
  });
});
