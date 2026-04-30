// ChainPage tests (REQ-CHN-003 / 004 / 005 / 007).
//
// Coverage targets, mapped to spec:
//   - Members render in chain order, Latest is emphasized
//   - Q&A history renders with status-correct UI (completed / failed / abandoned)
//     in a single pair-card render, including Re-ask affordances
//   - Submit flow: optimistic in-flight pair drops in below the active pair,
//     streams tokens, transitions on completed
//   - Submit clears the active textarea and refocuses it (so the user can
//     immediately type the next question)
//   - Inline name edit: click → edit → Enter commits via mocked PATCH; Esc cancels
//   - Snapshot staleness tag renders when current counts differ from snapshot
//   - 404 branch renders the empty state without crashing

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  render,
  screen,
  act,
  fireEvent,
  waitFor,
} from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import { ChainPage } from './ChainPage';
import type {
  ChainView,
  ChainQaRow,
  ChainMemberSummary,
  ChainSseEventData,
} from '../api';

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

type ChainEventHandler = (evt: ChainSseEventData) => void;

interface SseHandle {
  close: () => void;
  emit: (evt: ChainSseEventData) => void;
}

let sseHandles: SseHandle[] = [];

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      getChain: vi.fn(),
      submitChainQuestion: vi.fn(),
      setChainName: vi.fn(),
    },
    subscribeToChainStream: vi.fn(
      (_rootId: string, onEvent: ChainEventHandler) => {
        const handle: SseHandle = {
          close: vi.fn(),
          emit: (evt) => onEvent(evt),
        };
        sseHandles.push(handle);
        return {
          close: handle.close,
          // The actual EventSource has more methods, but ChainPage only calls
          // .close() on the return value. Cast through unknown for the test.
        } as unknown as EventSource;
      },
    ),
  };
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ROOT_ID = 'root-1';

const makeMember = (
  conv_id: string,
  position: ChainMemberSummary['position'],
  overrides: Partial<ChainMemberSummary> = {},
): ChainMemberSummary => ({
  conv_id,
  slug: `slug-${conv_id}`,
  title: `Title ${conv_id}`,
  message_count: 12,
  updated_at: '2026-04-29T12:00:00Z',
  position,
  ...overrides,
});

const makeQa = (
  id: string,
  overrides: Partial<ChainQaRow> = {},
): ChainQaRow => ({
  id,
  root_conv_id: ROOT_ID,
  question: `Q ${id}`,
  answer: `A ${id}`,
  model: 'sonnet-test',
  status: 'completed',
  snapshot_member_count: 2,
  snapshot_total_messages: 20,
  created_at: '2026-04-28T10:00:00Z',
  completed_at: '2026-04-28T10:00:30Z',
  ...overrides,
});

const makeChain = (overrides: Partial<ChainView> = {}): ChainView => ({
  root_conv_id: ROOT_ID,
  chain_name: 'auth refactor',
  display_name: 'auth refactor',
  members: [
    makeMember('m1', 'root'),
    makeMember('m2', 'continuation'),
    makeMember('m3', 'latest'),
  ],
  qa_history: [],
  current_member_count: 3,
  current_total_messages: 36,
  ...overrides,
});

function renderAt(rootId: string) {
  return render(
    <MemoryRouter initialEntries={[`/chains/${rootId}`]}>
      <Routes>
        <Route path="/chains/:rootConvId" element={<ChainPage />} />
        <Route path="/c/:slug" element={<div data-testid="conv-page">conv</div>} />
        <Route path="/" element={<div data-testid="home">home</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  sseHandles = [];
  vi.clearAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('ChainPage — members column', () => {
  it('renders members in chain order with the Latest member emphasized', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeChain());

    renderAt(ROOT_ID);

    await waitFor(() => {
      expect(screen.getByText('Title m1')).toBeInTheDocument();
    });
    expect(screen.getByText('Title m2')).toBeInTheDocument();
    expect(screen.getByText('Title m3')).toBeInTheDocument();

    // Order: root → continuation → latest, by DOM order in the list.
    const titles = screen
      .getAllByText(/^Title m\d$/)
      .map((el) => el.textContent);
    expect(titles).toEqual(['Title m1', 'Title m2', 'Title m3']);

    // Latest is emphasized via the "Latest" badge AND the .chain-member--latest
    // wrapper class.
    const latestCard = screen.getByText('Title m3').closest('.chain-member');
    expect(latestCard?.className).toContain('chain-member--latest');
    // The Latest badge is the .chain-member-badge sibling of the title.
    const badge = (latestCard as HTMLElement).querySelector(
      '.chain-member-badge',
    );
    expect(badge?.textContent).toBe('Latest');
  });
});

describe('ChainPage — Q&A history rendering', () => {
  it('renders persisted Q&A pairs in reverse-chronological order below the active pair', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      makeChain({
        qa_history: [
          makeQa('q-old', {
            question: 'oldest question',
            created_at: '2026-04-27T10:00:00Z',
          }),
          makeQa('q-mid', {
            question: 'middle question',
            created_at: '2026-04-28T10:00:00Z',
          }),
          makeQa('q-new', {
            question: 'newest question',
            created_at: '2026-04-29T10:00:00Z',
          }),
        ],
      }),
    );

    renderAt(ROOT_ID);

    await waitFor(() => {
      expect(screen.getByText('newest question')).toBeInTheDocument();
    });
    // The list contains the active pair at index 0, then persisted pairs in
    // reverse-chronological order: newest first, oldest last.
    const items = document.querySelectorAll('.chain-qa-list > li');
    expect(items.length).toBe(4);
    expect(items[0]?.querySelector('textarea')).not.toBeNull();
    expect(items[1]?.textContent).toContain('newest question');
    expect(items[2]?.textContent).toContain('middle question');
    expect(items[3]?.textContent).toContain('oldest question');
  });
});

describe('ChainPage — submit + stream', () => {
  it('drops the in-flight pair just below the active pair, streams tokens, then transitions on completed', async () => {
    const { api } = await import('../api');
    const initial = makeChain();
    const afterSubmit = makeChain({
      qa_history: [
        makeQa('qa-new', {
          question: 'What did we land?',
          answer: 'we landed X then Y',
          status: 'completed',
          snapshot_member_count: 3,
          snapshot_total_messages: 36,
          created_at: '2026-04-29T13:00:00Z',
        }),
      ],
    });
    const getChain = api.getChain as ReturnType<typeof vi.fn>;
    getChain.mockResolvedValueOnce(initial);
    getChain.mockResolvedValueOnce(afterSubmit);
    (api.submitChainQuestion as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      { chain_qa_id: 'qa-new' },
    );

    renderAt(ROOT_ID);

    // wait for initial chain to load (members visible)
    await waitFor(() => {
      expect(screen.getByText('Title m1')).toBeInTheDocument();
    });

    // Type into the active pair's textarea and submit.
    const textarea = screen.getByRole('textbox', { name: 'Question' });
    fireEvent.change(textarea, { target: { value: 'What did we land?' } });
    const submitBtn = screen.getByRole('button', { name: /Ask|Sending/ });
    fireEvent.click(submitBtn);

    // Optimistic in-flight: question is on screen, and its pair card sits
    // at index 1 of the .chain-qa-list (active pair is index 0).
    await waitFor(() => {
      expect(screen.getByText('What did we land?')).toBeInTheDocument();
    });
    const items = document.querySelectorAll('.chain-qa-list > li');
    expect(items.length).toBeGreaterThanOrEqual(2);
    // Index 0 is the active pair (contains the textarea).
    expect(items[0]?.querySelector('textarea')).not.toBeNull();
    // Index 1 is the just-submitted in-flight pair.
    expect(items[1]?.textContent).toContain('What did we land?');

    // Stream tokens via the captured SSE handle.
    expect(sseHandles).toHaveLength(1);
    act(() => {
      sseHandles[0]!.emit({
        type: 'chain_qa_token',
        chain_qa_id: 'qa-new',
        delta: 'we landed X ',
      });
      sseHandles[0]!.emit({
        type: 'chain_qa_token',
        chain_qa_id: 'qa-new',
        delta: 'then Y',
      });
    });
    // Streaming text appears live in the in-flight card.
    await waitFor(() => {
      expect(screen.getByText(/we landed X then Y/)).toBeInTheDocument();
    });

    // Completion: emit completed → ChainPage refetches → persisted answer shown.
    act(() => {
      sseHandles[0]!.emit({
        type: 'chain_qa_completed',
        chain_qa_id: 'qa-new',
        full_answer: 'we landed X then Y',
      });
    });

    // Refetch should have been called.
    await waitFor(() => {
      expect(getChain).toHaveBeenCalledTimes(2);
    });
    // Persisted card replaces the in-flight card (same answer text persists).
    await waitFor(() => {
      expect(screen.getByText(/we landed X then Y/)).toBeInTheDocument();
    });
    // After completion: the persisted pair sits at index 1 (just below
    // active), order in the panel did not move.
    const after = document.querySelectorAll('.chain-qa-list > li');
    expect(after[0]?.querySelector('textarea')).not.toBeNull();
    expect(after[1]?.textContent).toContain('we landed X then Y');
  });

  it('clears the active textarea and refocuses it after submit', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeChain());
    (api.submitChainQuestion as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      { chain_qa_id: 'qa-focus' },
    );

    renderAt(ROOT_ID);

    await waitFor(() => {
      expect(screen.getByText('Title m1')).toBeInTheDocument();
    });

    const textarea = screen.getByRole('textbox', {
      name: 'Question',
    }) as HTMLTextAreaElement;
    // The textarea should already be autofocused on mount.
    await waitFor(() => {
      expect(document.activeElement).toBe(textarea);
    });

    fireEvent.change(textarea, { target: { value: 'first question' } });
    fireEvent.click(screen.getByRole('button', { name: /Ask|Sending/ }));

    // After submit: textarea cleared, focused, and the in-flight pair is
    // visible at index 1 of the list.
    await waitFor(() => {
      expect(textarea.value).toBe('');
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(textarea);
    });
    expect(screen.getByText('first question')).toBeInTheDocument();
  });
});

describe('ChainPage — pair-card state matrix', () => {
  it('renders completed, failed, and abandoned pairs with the right re-ask affordances', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      makeChain({
        qa_history: [
          makeQa('q-completed', {
            question: 'completed question',
            answer: 'completed answer',
          }),
          makeQa('q-failed', {
            question: 'failed question',
            status: 'failed',
            answer: 'partial answer',
            completed_at: null,
          }),
          makeQa('q-abandoned', {
            question: 'abandoned question',
            status: 'abandoned',
            answer: null,
            completed_at: null,
          }),
        ],
      }),
    );

    renderAt(ROOT_ID);

    await waitFor(() => {
      expect(screen.getByText('completed answer')).toBeInTheDocument();
    });

    // All three persisted pairs render. The active pair is index 0.
    const items = document.querySelectorAll('.chain-qa-list > li');
    expect(items.length).toBe(4);
    expect(items[0]?.querySelector('textarea')).not.toBeNull();

    // Failed: shows partial + Failed label + Re-ask button.
    expect(screen.getByText('partial answer')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
    // Abandoned: shows "Did not complete" + Re-ask button.
    expect(screen.getByText('Did not complete')).toBeInTheDocument();
    // Two Re-ask buttons (failed + abandoned).
    const reaskButtons = screen.getAllByRole('button', { name: 'Re-ask' });
    expect(reaskButtons).toHaveLength(2);

    // Re-ask on a failed/abandoned pair populates the active textarea with
    // the original question and keeps focus there. (No auto-submit — user
    // agency, REQ-CHN-007 precedent.) Click the second Re-ask button (which
    // belongs to the failed pair — pairs render in reverse-chronological
    // order so the abandoned pair, last in the qa_history input, appears
    // first in DOM order).
    const submitMock = api.submitChainQuestion as ReturnType<typeof vi.fn>;
    submitMock.mockReset();
    fireEvent.click(reaskButtons[1]!);
    const textarea = screen.getByRole('textbox', {
      name: 'Question',
    }) as HTMLTextAreaElement;
    await waitFor(() => {
      expect(textarea.value).toBe('failed question');
    });
    expect(submitMock).not.toHaveBeenCalled();
  });
});

describe('ChainPage — inline name edit (REQ-CHN-007)', () => {
  it('Enter commits via setChainName; Esc cancels without an API call', async () => {
    const { api } = await import('../api');
    const initial = makeChain({ chain_name: 'old-name', display_name: 'old-name' });
    const renamed = makeChain({ chain_name: 'new-name', display_name: 'new-name' });
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(initial);
    (api.setChainName as ReturnType<typeof vi.fn>).mockResolvedValueOnce(renamed);

    renderAt(ROOT_ID);

    // Wait for header to render.
    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: /old-name/ }),
      ).toBeInTheDocument();
    });

    // Click the name → input appears pre-populated with the override.
    fireEvent.click(screen.getByRole('button', { name: /old-name/ }));
    const input = screen.getByRole('textbox', { name: 'Chain name' });
    expect(input).toHaveValue('old-name');

    // Esc cancels: nothing committed, button reappears unchanged.
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(api.setChainName).not.toHaveBeenCalled();
    expect(
      screen.getByRole('button', { name: /old-name/ }),
    ).toBeInTheDocument();

    // Enter into edit again, change, Enter commits.
    fireEvent.click(screen.getByRole('button', { name: /old-name/ }));
    const input2 = screen.getByRole('textbox', { name: 'Chain name' });
    fireEvent.change(input2, { target: { value: 'new-name' } });
    fireEvent.keyDown(input2, { key: 'Enter' });

    await waitFor(() => {
      expect(api.setChainName).toHaveBeenCalledWith(ROOT_ID, 'new-name');
    });
    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: /new-name/ }),
      ).toBeInTheDocument();
    });
  });

  it('clearing the input commits null (clear override)', async () => {
    const { api } = await import('../api');
    const initial = makeChain({ chain_name: 'old', display_name: 'old' });
    const cleared = makeChain({ chain_name: null, display_name: 'fallback-title' });
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(initial);
    (api.setChainName as ReturnType<typeof vi.fn>).mockResolvedValueOnce(cleared);

    renderAt(ROOT_ID);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /old/ })).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole('button', { name: /old/ }));
    const input = screen.getByRole('textbox', { name: 'Chain name' });
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => {
      expect(api.setChainName).toHaveBeenCalledWith(ROOT_ID, null);
    });
  });
});

describe('ChainPage — snapshot staleness (REQ-CHN-005)', () => {
  it('renders a staleness tag when member count differs', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      makeChain({
        qa_history: [
          makeQa('q-stale', {
            snapshot_member_count: 2,
            snapshot_total_messages: 20,
          }),
        ],
        // current state advanced
        current_member_count: 4,
        current_total_messages: 50,
      }),
    );

    renderAt(ROOT_ID);
    // The tag prefers the member-count phrasing when both differ.
    await waitFor(() => {
      expect(
        screen.getByText(/answered when chain had 2 conversations \(now 4\)/),
      ).toBeInTheDocument();
    });
  });

  it('renders a message-count-based tag when only message count differs', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      makeChain({
        qa_history: [
          makeQa('q-msg-stale', {
            snapshot_member_count: 3, // matches current
            snapshot_total_messages: 18,
          }),
        ],
        current_member_count: 3,
        current_total_messages: 27,
      }),
    );

    renderAt(ROOT_ID);
    await waitFor(() => {
      expect(
        screen.getByText(/answered with 18 prior messages \(now 27\)/),
      ).toBeInTheDocument();
    });
  });

  it('does not render a tag when current state matches snapshot', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      makeChain({
        qa_history: [
          makeQa('q-fresh', {
            snapshot_member_count: 3,
            snapshot_total_messages: 36,
          }),
        ],
        current_member_count: 3,
        current_total_messages: 36,
      }),
    );

    renderAt(ROOT_ID);
    await waitFor(() => {
      expect(screen.getByText('A q-fresh')).toBeInTheDocument();
    });
    expect(screen.queryByText(/answered when/)).not.toBeInTheDocument();
    expect(screen.queryByText(/answered with/)).not.toBeInTheDocument();
  });
});

describe('ChainPage — 404 / not-a-chain branch', () => {
  it('renders the not-a-chain empty state with a back link without crashing', async () => {
    const { api } = await import('../api');
    (api.getChain as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error('Chain not found'),
    );

    renderAt('not-a-chain-id');

    await waitFor(() => {
      expect(screen.getByText('Not a chain')).toBeInTheDocument();
    });
    const back = screen.getByRole('button', { name: /Back to conversations/ });
    expect(back).toBeInTheDocument();
    fireEvent.click(back);
    await waitFor(() => {
      expect(screen.getByTestId('home')).toBeInTheDocument();
    });
  });
});
