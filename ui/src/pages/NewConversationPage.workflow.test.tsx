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
      cwd: '/repo',
      objective: 'Ship it',
      model: 'claude-3-5-sonnet',
      images: [],
      settings: {},
    });
    expect(firstRequest).toHaveProperty('message_id');
    expect(firstRequest).toHaveProperty('conversation_id');
    expect(firstRequest).not.toHaveProperty('mode');
    expect(firstRequest).not.toHaveProperty('base_branch');
    expect(firstRequest).not.toHaveProperty('branch');
    expect(firstRequest).not.toHaveProperty('checkout_ref');
    expect(firstRequest).not.toHaveProperty('project_id');
    expect(firstRequest).not.toHaveProperty('task');
    expect(screen.getByTestId('location-path')).toHaveTextContent('/product-conversations/pc-1');

    vi.mocked(api.createProductConversation).mockClear();
    firstPage.unmount();
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: false });
    localStorage.setItem('phoenix-last-cwd', '/plain-dir');
    renderPage();

    await screen.findAllByPlaceholderText('What would you like to work on?');
    fireEvent.change(composerTextarea(), { target: { value: 'Ship it' } });
    fireEvent.click(sendButton());

    await waitFor(() => expect(api.createProductConversation).toHaveBeenCalledTimes(1));
    const secondRequest = vi.mocked(api.createProductConversation).mock.calls[0]![0];
    expect(secondRequest).toMatchObject({
      cwd: '/plain-dir',
      objective: 'Ship it',
      model: 'claude-3-5-sonnet',
      images: [],
      settings: {},
    });
    expect(Object.keys(secondRequest).sort()).toEqual(Object.keys(firstRequest).sort());
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
    expect(screen.getByTestId('location-path')).toHaveTextContent('/product-conversations/pc-1');
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
