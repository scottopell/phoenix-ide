import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';
import { ConversationProvider } from '../conversation';
import { api } from '../api';

vi.mock('../api', () => {
  class ExpansionError extends Error {
    detail: { error: string };

    constructor(error: string) {
      super(error);
      this.detail = { error };
    }
  }

  return {
    ExpansionError,
    api: {
      listModels: vi.fn(),
      getEnv: vi.fn(),
      validateCwd: vi.fn(),
      listDirectory: vi.fn(),
      listProjectSkills: vi.fn(),
      searchProjectFiles: vi.fn(),
      createProductConversation: vi.fn(),
      reserveProductRoot: vi.fn(),
      listRecentManagementRootSuggestions: vi.fn(),
      mkdir: vi.fn(),
      listConversations: vi.fn().mockResolvedValue([]),
      listArchivedConversations: vi.fn().mockResolvedValue([]),
    },
  };
});

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn().mockResolvedValue([]),
    syncConversations: vi.fn().mockResolvedValue(undefined),
    putConversation: vi.fn().mockResolvedValue(undefined),
  },
}));

function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location-path">{location.pathname}</div>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/new']}>
      <ConversationProvider>
        <LocationProbe />
        <NewConversationPage />
      </ConversationProvider>
    </MemoryRouter>,
  );
}

function composerTextarea() {
  return screen.getAllByPlaceholderText('What would you like to work on?')[0] as HTMLTextAreaElement;
}

function sendButton() {
  return screen.getAllByRole('button', { name: 'Send' })[0] as HTMLButtonElement;
}

const modelResponse = {
  models: [{ id: 'claude-3-5-sonnet', provider: 'anthropic', recommended: true, description: '', context_window: 200_000, effort_capabilities: { support: 'supported', levels: ['low', 'high'], native_default: { known: 'high' } } }],
  default: 'claude-3-5-sonnet',
  llm_configured: true,
  credential_status: 'valid',
};

describe('/new directory-first product conversation', () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/repo');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
    vi.mocked(api.listModels).mockResolvedValue(modelResponse as never);
    vi.mocked(api.getEnv).mockResolvedValue({ home_dir: '/home/user' });
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: true });
    vi.mocked(api.listDirectory).mockResolvedValue({ entries: [] });
    vi.mocked(api.listProjectSkills).mockResolvedValue({ skills: [] });
    vi.mocked(api.searchProjectFiles).mockResolvedValue({ items: [] });
    vi.mocked(api.listRecentManagementRootSuggestions).mockResolvedValue({ suggestions: [] });
    vi.mocked(api.reserveProductRoot).mockResolvedValue({
      kind: 'exact_committed_tree',
      exact_checkout_oid: 'abc123',
      logical_base: 'main',
      freshness: 'fresh',
      root_reservation: {
        repository_id: null, id: 'reservation-git', cwd: '/repo', kind: 'exact_committed_tree', repo_root: '/repo', exact_checkout_oid: 'abc123',
        logical_base: 'main', freshness: 'fresh', unresolved_reason: null,
      },
    });
    vi.mocked(api.mkdir).mockResolvedValue({ created: true });
    vi.mocked(api.createProductConversation).mockResolvedValue({
      product_conversation_id: 'pc-1',
      canonical_route: '/product-conversations/pc-1',
    } as never);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('submits the same typed request for git and non-git directories', async () => {
    const firstPage = renderPage();

    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Ship it' } });
    fireEvent.click(sendButton());

    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(1));
    const firstRequest = vi.mocked(api.createProductConversation).mock.calls[0]![0];
    expect(firstRequest).toMatchObject({
      root_reservation: expect.objectContaining({ cwd: '/repo', exact_checkout_oid: 'abc123' }),
      objective: 'Ship it',
      model: 'claude-3-5-sonnet',
      images: [],
    });
    expect(firstRequest).toHaveProperty('message_id');
    expect(firstRequest).toHaveProperty('conversation_id');
    expect(firstRequest).not.toHaveProperty('mode');
    expect(firstRequest).not.toHaveProperty('base_branch');
    expect(firstRequest).not.toHaveProperty('branch');
    expect(firstRequest).not.toHaveProperty('checkout_ref');
    expect(firstRequest).not.toHaveProperty('project_id');
    expect(firstRequest).not.toHaveProperty('task');
    await waitFor(() => expect(screen.getByTestId('location-path')).toHaveTextContent('/product-conversations/pc-1'));

    vi.mocked(api.createProductConversation).mockClear();
    firstPage.unmount();
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    vi.mocked(api.reserveProductRoot).mockResolvedValue({
      kind: 'direct',
      root_reservation: {
        repository_id: null, id: 'reservation-direct', cwd: '/plain-dir', kind: 'direct', repo_root: null, exact_checkout_oid: null,
        logical_base: null, freshness: null, unresolved_reason: null,
      },
    });
    localStorage.setItem('phoenix-last-cwd', '/plain-dir');
    renderPage();

    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Ship it' } });
    fireEvent.click(sendButton());

    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(1));
    const secondRequest = vi.mocked(api.createProductConversation).mock.calls[0]![0];
    expect(secondRequest).toMatchObject({
      root_reservation: expect.objectContaining({ cwd: '/plain-dir', kind: 'direct' }),
      objective: 'Ship it',
      model: 'claude-3-5-sonnet',
      images: [],
    });
    expect(Object.keys(secondRequest).sort()).toEqual(Object.keys(firstRequest).sort());
  });

  it('creates a missing directory before reserving and submitting its canonical root', async () => {
    localStorage.setItem('phoenix-last-cwd', '/new-dir');
    vi.mocked(api.validateCwd).mockImplementation(async (path) => (
      path === '/new-dir'
        ? { valid: false } as never
        : { valid: true, is_git: false }
    ));
    vi.mocked(api.reserveProductRoot).mockResolvedValue({
      kind: 'direct',
      root_reservation: {
        repository_id: null, id: 'reservation-created', cwd: '/new-dir', kind: 'direct', repo_root: null,
        exact_checkout_oid: null, logical_base: null, freshness: null, unresolved_reason: null,
      },
    });
    const order: string[] = [];
    vi.mocked(api.mkdir).mockImplementation(async () => { order.push('mkdir'); return { created: true }; });
    vi.mocked(api.reserveProductRoot).mockImplementation(async () => {
      order.push('reserve');
      return {
        kind: 'direct',
        root_reservation: {
          repository_id: null, id: 'reservation-created', cwd: '/new-dir', kind: 'direct', repo_root: null,
          exact_checkout_oid: null, logical_base: null, freshness: null, unresolved_reason: null,
        },
      };
    });
    vi.mocked(api.createProductConversation).mockImplementation(async () => {
      order.push('create');
      return { product_conversation_id: 'pc-created', canonical_route: '/product-conversations/pc-created' } as never;
    });

    renderPage();
    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Create here' } });
    await waitFor(() => expect(screen.getAllByTitle('Directory will be created').length).toBeGreaterThan(0));
    fireEvent.click(sendButton());

    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(1));
    expect(order.slice(-3)).toEqual(['mkdir', 'reserve', 'create']);
    expect(vi.mocked(api.createProductConversation).mock.calls[0]![0].root_reservation.id)
      .toBe('reservation-created');
  });

  it('reacquires an expired reservation while preserving create identities', async () => {
    vi.mocked(api.createProductConversation)
      .mockRejectedValueOnce(new Error('invalid product root reservation'))
      .mockResolvedValueOnce({ product_conversation_id: 'pc-refreshed', canonical_route: '/product-conversations/pc-refreshed' } as never);
    vi.mocked(api.reserveProductRoot)
      .mockResolvedValueOnce({
        kind: 'exact_committed_tree', exact_checkout_oid: 'abc123', logical_base: 'main', freshness: 'fresh',
        root_reservation: {
          repository_id: null, id: 'reservation-expired', cwd: '/repo', kind: 'exact_committed_tree', repo_root: '/repo',
          exact_checkout_oid: 'abc123', logical_base: 'main', freshness: 'fresh', unresolved_reason: null,
        },
      })
      .mockResolvedValueOnce({
        kind: 'exact_committed_tree', exact_checkout_oid: 'def456', logical_base: 'main', freshness: 'fresh',
        root_reservation: {
          repository_id: null, id: 'reservation-refreshed', cwd: '/repo', kind: 'exact_committed_tree', repo_root: '/repo',
          exact_checkout_oid: 'def456', logical_base: 'main', freshness: 'fresh', unresolved_reason: null,
        },
      });

    renderPage();
    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Retry expired root' } });
    fireEvent.click(sendButton());
    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(2));
    const first = vi.mocked(api.createProductConversation).mock.calls[0]![0];
    const second = vi.mocked(api.createProductConversation).mock.calls[1]![0];
    expect(second.root_reservation.id).toBe('reservation-refreshed');
    expect(second.conversation_id).toBe(first.conversation_id);
    expect(second.message_id).toBe(first.message_id);
  });

  it('reuses create identities after an uncertain response', async () => {
    vi.mocked(api.createProductConversation)
      .mockRejectedValueOnce(new Error('network response lost'))
      .mockResolvedValueOnce({ product_conversation_id: 'pc-reused', canonical_route: '/product-conversations/pc-reused' } as never);

    renderPage();
    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Retry safely' } });
    fireEvent.click(sendButton());
    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(1));
    fireEvent.click(sendButton());
    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(2));

    const first = vi.mocked(api.createProductConversation).mock.calls[0]![0];
    const second = vi.mocked(api.createProductConversation).mock.calls[1]![0];
    expect(second.conversation_id).toBe(first.conversation_id);
    expect(second.message_id).toBe(first.message_id);
  });

  it('keeps the draft on failure so retry is truthful', async () => {
    vi.mocked(api.createProductConversation)
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce({ product_conversation_id: 'pc-1', canonical_route: '/product-conversations/pc-1' } as never);

    renderPage();
    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Draft survives' } });
    fireEvent.click(sendButton());

    await waitFor(() => expect(screen.getAllByText('boom').length).toBeGreaterThan(0));
    expect(composerTextarea()).toHaveValue('Draft survives');

    fireEvent.click(sendButton());
    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('location-path')).toHaveTextContent('/product-conversations/pc-1'));
  });

  it('does not request project, branch, or task metadata', async () => {
    renderPage();
    await screen.findAllByPlaceholderText('What would you like to work on?');

    expect(screen.queryByText('Workflow')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Suggested projects')).not.toBeInTheDocument();
    expect(api.listProjectSkills).not.toHaveBeenCalled();
    expect(api.searchProjectFiles).not.toHaveBeenCalled();
  });
});
