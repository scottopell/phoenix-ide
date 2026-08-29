import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';
import { ConversationProvider } from '../conversation';

const { apiMock } = vi.hoisted(() => ({ apiMock: {
  listModels: vi.fn().mockResolvedValue({ models: [{ id: 'claude-3-5-sonnet' }], default: 'claude-3-5-sonnet', llm_configured: true }),
  getEnv: vi.fn().mockResolvedValue({ home_dir: '/home/user' }),
  listRecentManagementRootSuggestions: vi.fn().mockResolvedValue({ suggestions: [] }),
  getProjects: vi.fn().mockResolvedValue([]),
  validateCwd: vi.fn().mockResolvedValue({ valid: true }),
  listDirectory: vi.fn().mockResolvedValue({ entries: [] }),
  listConversations: vi.fn().mockResolvedValue([]),
  listArchivedConversations: vi.fn().mockResolvedValue([]),
  listProductConversationCreations: vi.fn().mockResolvedValue({ product_creations: [] }),
  cancelProductConversationCreation: vi.fn().mockResolvedValue(undefined),
  retryProductConversationDelivery: vi.fn().mockResolvedValue(undefined),
  deleteProductConversationCreation: vi.fn().mockResolvedValue(undefined),
} }));

vi.mock('../api', () => ({
  api: apiMock,
  ExpansionError: class ExpansionError extends Error {},
}));

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn().mockResolvedValue([]),
    syncConversations: vi.fn().mockResolvedValue(undefined),
    putConversation: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock('../hooks/useMediaQuery', () => ({ useIsDesktop: () => false }));
vi.mock('../hooks', async () => {
  const actual = await vi.importActual('../hooks');
  return {
    ...actual,
    useInlineReferences: () => ({
      dropdown: null,
      onValueChange: vi.fn(),
      onKeyDown: vi.fn(() => false),
      onSelectionChange: vi.fn(),
      reset: vi.fn(),
      expansionError: null,
      setExpansionError: vi.fn(),
      skillArgumentHint: null,
    }),
  };
});

describe('NewConversationPage', () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/home/user/projects');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
    Object.values(apiMock).forEach((fn) => {
      if ('mockClear' in fn) fn.mockClear();
    });
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [] });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  function renderPage() {
    return render(
      <MemoryRouter>
        <ConversationProvider>
          <NewConversationPage />
        </ConversationProvider>
      </MemoryRouter>,
    );
  }

  it('does not show checking status on initial render when cwd is saved', () => {
    const { container } = renderPage();
    expect(container.querySelectorAll('.status-checking').length).toBe(0);
  });

  it('renders recovery actions and wires retry/cancel/delete', async () => {
    apiMock.listProductConversationCreations
      .mockResolvedValueOnce({
        product_creations: [
          { request_id: 'req-pending', status: 'accepted', cwd: '/repo/pending', objective: 'pending objective', model: 'claude', effort: null, updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['cancel', 'start_over'] },
          { request_id: 'req-delivery', status: 'delivery_failed', cwd: '/repo/delivery', objective: 'delivery objective', model: 'claude', effort: 'high', updated_at: '2025-01-01T00:00:00Z', last_error: 'delivery failed', allowed_actions: ['retry_delivery', 'start_over'] },
          { request_id: 'req-failed', status: 'failed', cwd: '/repo/failed', objective: 'failed objective', model: 'claude', effort: null, updated_at: '2025-01-01T00:00:00Z', last_error: 'boom', allowed_actions: ['delete', 'start_over'] },
        ],
      })
      .mockResolvedValue({ product_creations: [] });

    renderPage();

    expect((await screen.findAllByText('Recent starts')).length).toBeGreaterThan(0);
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel' }).at(0)!);
    await waitFor(() => expect(apiMock.cancelProductConversationCreation).toHaveBeenCalledWith('req-pending'));

    apiMock.listProductConversationCreations.mockResolvedValueOnce({ product_creations: [] });
    renderPage();
  });

  it('start over prefills the composer from a recovery row', async () => {
    apiMock.listProductConversationCreations.mockResolvedValueOnce({
      product_creations: [
        { request_id: 'req-failed', status: 'failed', cwd: '/repo/failed', objective: 'redo this', model: 'claude-3-5-sonnet', effort: 'high', updated_at: '2025-01-01T00:00:00Z', last_error: 'boom', allowed_actions: ['delete', 'start_over'] },
      ],
    });

    renderPage();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Start over' })).at(0)!);
    await waitFor(() => expect(screen.getAllByDisplayValue('redo this').length).toBeGreaterThan(0));
  });
});
