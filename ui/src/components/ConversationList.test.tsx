// Issue regression tests for ConversationList component.
//
// SIDE-02: "All" tab has no project labels
// SIDE-04: Context menu persists across navigation (no click-outside handler)
// CHN-Sidebar: chain grouping render (REQ-CHN-002, task 02690 Phase 5)

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, within, act, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import type { ProductConversationListRow } from '../api';
import { api } from '../api';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import { ShortcutHelpPanel } from './ShortcutHelpPanel';

// Spy on a util that the row body calls during render. Counting these calls
// is a reliable proxy for component-body executions: when React.memo bails
// out, the component body does not run, so the spy is not invoked. We assert
// "spy was called" vs "not called" rather than absolute counts, since the
// row currently calls formatRelativeTime in both the displayed text and the
// title attribute (2 invocations per row render today). Memoization is
// "did the body run at all," which the boolean-style assertion captures
// without coupling to the call-per-render constant.
//
// This dodges the Profiler false-positive (Profiler.onRender fires per
// commit in the profiled subtree even when a child memoizes).
//
// vi.hoisted ensures the spy variables exist when vi.mock's factory runs
// (vitest hoists vi.mock calls above module-level `const` declarations).
const { formatRelativeTimeSpy, formatShortDateTimeSpy } = vi.hoisted(() => ({
  formatRelativeTimeSpy: vi.fn((iso: string) => `rel-${iso}`),
  formatShortDateTimeSpy: vi.fn((iso: string) => `short-${iso}`),
}));
vi.mock('../utils', async () => {
  const actual = await vi.importActual<typeof import('../utils')>('../utils');
  return {
    ...actual,
    formatRelativeTime: formatRelativeTimeSpy,
    formatShortDateTime: formatShortDateTimeSpy,
  };
});

import { ConversationList } from './ConversationList';
import { productConversationPresentationIndicator } from './ConversationList.presentation';

describe('ConversationList — pinned header scroll offset', () => {
  it('updates the nearest app scroll owner across densities and restores its prior inline style', () => {
    const observers: Array<{ observed: Element[]; disconnected: boolean; trigger: () => void }> = [];
    const OriginalResizeObserver = globalThis.ResizeObserver;
    class ObserverMock {
      observed: Element[] = [];
      disconnected = false;
      constructor(private callback: ResizeObserverCallback) { observers.push(this); }
      observe(target: Element) { this.observed.push(target); }
      unobserve() {}
      disconnect() { this.disconnected = true; }
      trigger() { this.callback([], this as unknown as ResizeObserver); }
    }
    vi.stubGlobal('ResizeObserver', ObserverMock);
    try {
      const { container, rerender, unmount } = render(
        <MemoryRouter><div data-app-scroll-owner style={{ scrollPaddingTop: '7px' }}>
          <ConversationList {...defaultProps} listDensity="mobile" />
        </div></MemoryRouter>,
      );
      const owner = container.querySelector<HTMLElement>('[data-app-scroll-owner]')!;
      const root = container.querySelector<HTMLElement>('#conversation-list')!;
      const header = root.querySelector<HTMLElement>('.conversation-list-pinned-header')!;
      vi.spyOn(header, 'getBoundingClientRect').mockReturnValue({ height: 48 } as DOMRect);
      act(() => observers.at(-1)!.trigger());
      expect(owner.style.scrollPaddingTop).toBe('48px');
      expect(root.style.scrollPaddingTop).toBe('');

      rerender(<MemoryRouter><div data-app-scroll-owner style={{ scrollPaddingTop: '7px' }}>
        <ConversationList {...defaultProps} listDensity="full" />
      </div></MemoryRouter>);
      expect(observers[0]!.disconnected).toBe(true);
      expect(owner.style.scrollPaddingTop).toBe('48px');
      rerender(<MemoryRouter><div data-app-scroll-owner style={{ scrollPaddingTop: '7px' }}>
        <ConversationList {...defaultProps} sidebarMode listDensity="sidebar" />
      </div></MemoryRouter>);
      expect(observers.at(-1)!.observed).toContain(header);
      unmount();
      expect(owner.style.scrollPaddingTop).toBe('7px');
      expect(observers.at(-1)!.disconnected).toBe(true);
    } finally {
      vi.stubGlobal('ResizeObserver', OriginalResizeObserver);
    }
  });
});

describe('ConversationList — global navigation', () => {
  it('pins Ember below the complete mobile list toolbar and navigates with router state', () => {
    function Location() { return <span data-testid="location">{useLocation().pathname}</span>; }
    const { getByRole, getByTestId, container } = render(
      <MemoryRouter initialEntries={['/']}>
        <ConversationList {...defaultProps} listDensity="mobile" />
        <Location />
      </MemoryRouter>,
    );

    const pinnedHeader = container.querySelector('.conversation-list-pinned-header')!;
    const toolbar = pinnedHeader.querySelector('.conversation-list-header')!;
    const ember = getByRole('button', { name: 'Open Ember' });
    expect(ember).toHaveAccessibleDescription('Global coordinator');
    expect(pinnedHeader.firstElementChild).toBe(toolbar);
    expect(toolbar.contains(ember)).toBe(false);
    expect(toolbar.nextElementSibling).toBe(ember);
    expect(pinnedHeader).toContainElement(ember);

    fireEvent.click(ember);
    expect(getByTestId('location')).toHaveTextContent('/global');
    expect(ember).not.toHaveAttribute('aria-current');
  });

  it('leaves list navigation when opening Ember through the mobile route boundary', () => {
    const { getByRole, queryByRole } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="/" element={<ConversationList {...defaultProps} listDensity="mobile" />} />
          <Route path="/global" element={<h1>Ember destination</h1>} />
        </Routes>
      </MemoryRouter>,
    );

    fireEvent.click(getByRole('button', { name: 'Open Ember' }));
    expect(getByRole('heading', { name: 'Ember destination' })).toBeInTheDocument();
    expect(queryByRole('button', { name: 'Open Ember' })).not.toBeInTheDocument();
  });
});



const makeProductConversation = (id: string, overrides: Partial<ProductConversationListRow> = {}): ProductConversationListRow => ({
  product_conversation_id: id,
  canonical_route: `/product-conversations/${id}`,
  canonical_root: {
    transcript_row_id: `root-${id}`,
    slug: `root-${id}`,
    title: `Root ${id}`,
  },
  lifecycle: { state: 'open', close_action: { availability: 'available' } },
  latest_transcript_row_id: `latest-${id}`,
  updated_at: '2024-01-01T00:00:00Z',
  presentation: { kind: 'state', display_name: overrides.canonical_root?.title ?? `Root ${id}`, presentation_mode: 'idle' },
  read_state: { state: 'read' },
  cached_pr: null,
  ...overrides,
});

describe('ProductConversation presentation indicator', () => {
  it.each([
    ['needs_action kind', makeProductConversation('needs-kind', { presentation: { kind: 'needs_action', display_name: 'Needs Kind' } }), 'Needs action', 'awaiting-approval'],
    ['awaiting user response mode', makeProductConversation('awaiting-mode', { presentation: { kind: 'state', display_name: 'Awaiting', presentation_mode: 'needs_action' } }), 'Needs action', 'awaiting-approval'],
    ['working mode', makeProductConversation('working-mode', { presentation: { kind: 'state', display_name: 'Working', presentation_mode: 'working' } }), 'Working', 'working'],
    ['error mode', makeProductConversation('error-mode', { presentation: { kind: 'state', display_name: 'Error', presentation_mode: 'error' } }), 'Error', 'error'],
    ['done mode', makeProductConversation('done-mode', { presentation: { kind: 'state', display_name: 'Done', presentation_mode: 'done' } }), 'Completed', 'terminal'],
    ['history idle', makeProductConversation('history-idle', { lifecycle: { state: 'history' }, presentation: { kind: 'state', display_name: 'History', presentation_mode: 'idle' } }), 'History', 'terminal'],
  ])('%s maps to one authoritative indicator', (_name, row, label, dotClass) => {
    expect(productConversationPresentationIndicator(row)).toEqual({ label, ariaLabel: label, dotClass });
  });

  it('renders one indicator element for a continued product conversation row', () => {
    const row = makeProductConversation('continued-indicator', {
      canonical_root: { transcript_row_id: 'root-row', slug: 'root-slug', title: 'Root Title' },
      latest_transcript_row_id: 'latest-row',
      presentation: { kind: 'state', display_name: 'Awaiting User', presentation_mode: 'needs_action' },
      read_state: { state: 'read' },
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} productConversations={[row]} />
      </MemoryRouter>,
    );

    const rowNode = container.querySelector('[data-product-conversation-id="continued-indicator"]');
    expect(rowNode).not.toBeNull();
    expect(rowNode!.querySelectorAll('.conv-state-dot')).toHaveLength(1);
    expect(within(rowNode as HTMLElement).getByLabelText('Needs action')).toBeInTheDocument();
  });
});

describe('ProductConversation presentation transitions', () => {
  it('rerenders working to needs action to done with exactly one indicator', () => {
    const base = makeProductConversation('transition-row', { presentation: { kind: 'state', display_name: 'Working', presentation_mode: 'working' } });
    const { container, rerender } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} productConversations={[base]} />
      </MemoryRouter>,
    );
    const row = () => container.querySelector('[data-product-conversation-id="transition-row"]') as HTMLElement;
    expect(row().querySelectorAll('.conv-state-dot')).toHaveLength(1);
    expect(within(row()).getByLabelText('Working')).toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <ConversationList {...defaultProps} productConversations={[{ ...base, presentation: { kind: 'state', display_name: 'Needs Action', presentation_mode: 'needs_action' } }]} />
      </MemoryRouter>,
    );
    expect(row().querySelectorAll('.conv-state-dot')).toHaveLength(1);
    expect(within(row()).getByLabelText('Needs action')).toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <ConversationList {...defaultProps} productConversations={[{ ...base, presentation: { kind: 'state', display_name: 'Done', presentation_mode: 'done' } }]} />
      </MemoryRouter>,
    );
    expect(row().querySelectorAll('.conv-state-dot')).toHaveLength(1);
    expect(within(row()).getByLabelText('Completed')).toBeInTheDocument();
  });
});

describe('ProductConversation row actions', () => {
  it('leaves Escape to the actual topmost shortcut help panel', () => {
    const closeHelp = vi.fn();
    const content = (visible: boolean) => <FocusScopeProvider><MemoryRouter>
      <ConversationList {...defaultProps} productConversations={[makeProductConversation('under-panel')]} onProductConversationRename={vi.fn()} />
      <ShortcutHelpPanel visible={visible} onClose={closeHelp} />
    </MemoryRouter></FocusScopeProvider>;
    const view = render(content(false));
    const trigger = view.getByRole('button', { name: 'Actions for conversation Root under-panel' });
    fireEvent.click(trigger);
    view.rerender(content(true));
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(closeHelp).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    view.rerender(content(false));
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(trigger).toHaveFocus();
  });

  it('offers only the read-state action for offline History or mismatched callbacks', () => {
    const view = render(<MemoryRouter><ConversationList {...defaultProps} showArchived archivedProductConversations={[makeProductConversation('empty', { lifecycle: { state: 'history' } })]} onProductConversationRename={vi.fn()} onProductConversationClose={vi.fn()} /></MemoryRouter>);
    fireEvent.click(view.getByRole('button', { name: /^Actions for conversation/ }));
    const actions = view.getByLabelText(/^Actions for Root empty/);
    expect(within(actions).getAllByRole('button').map((button) => button.textContent)).toEqual(['Mark as unread']);
  });

  it('shows unread state inline and toggles it from the row menu', async () => {
    const markRead = vi.spyOn(api, 'markProductConversationRead').mockResolvedValue({ state: 'read' });
    const markUnread = vi.spyOn(api, 'markProductConversationUnread').mockResolvedValue({ state: 'unread', reason: 'marked' });
    const view = render(<MemoryRouter><ConversationList {...defaultProps} productConversations={[
      makeProductConversation('fresh', { read_state: { state: 'unread', reason: 'new_activity' } }),
      makeProductConversation('marked', { read_state: { state: 'unread', reason: 'marked' } }),
      makeProductConversation('seen'),
    ]} /></MemoryRouter>);
    expect(view.getByRole('img', { name: 'New activity' })).toBeTruthy();
    expect(view.getByRole('img', { name: 'Marked unread' }).className).toContain('conv-item-unread-dot--marked');
    expect(view.container.querySelectorAll('.product-conversation-list-row.unread')).toHaveLength(2);

    fireEvent.click(view.getByRole('button', { name: 'Actions for conversation Root fresh' }));
    fireEvent.click(view.getByRole('button', { name: 'Mark as read: Root fresh' }));
    await waitFor(() => expect(markRead).toHaveBeenCalledWith('fresh', 'acknowledged'));

    fireEvent.click(view.getByRole('button', { name: 'Actions for conversation Root seen' }));
    fireEvent.click(view.getByRole('button', { name: 'Mark as unread: Root seen' }));
    await waitFor(() => expect(markUnread).toHaveBeenCalledWith('seen'));
    markRead.mockRestore();
    markUnread.mockRestore();
  });

  it('dismisses after focus leaves the menu but lets a closed trigger Escape bubble', () => {
    const bubble = vi.fn();
    const view = render(<MemoryRouter><div onKeyDown={bubble}><button>Outside focus</button><ConversationList {...defaultProps} productConversations={[makeProductConversation('escape')]} onProductConversationRename={vi.fn()} /></div></MemoryRouter>);
    const trigger = view.getByRole('button', { name: 'Actions for conversation Root escape' });
    fireEvent.click(trigger);
    const outside = view.getByRole('button', { name: 'Outside focus' });
    outside.focus();
    fireEvent.keyDown(outside, { key: 'Escape' });
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(bubble).not.toHaveBeenCalled();
    fireEvent.keyDown(trigger, { key: 'Escape' });
    expect(bubble).toHaveBeenCalledTimes(1);
  });

  it('restores list keyboard navigation when the expanded row disappears', () => {
    const open = vi.fn();
    const first = makeProductConversation('first');
    const second = makeProductConversation('second');
    const props = { ...defaultProps, onProductConversationClick: open, onProductConversationRename: vi.fn() };
    const view = render(<MemoryRouter><ConversationList {...props} productConversations={[first, second]} /></MemoryRouter>);
    fireEvent.click(view.getByRole('button', { name: 'Actions for conversation Root first' }));
    view.rerender(<MemoryRouter><ConversationList {...props} productConversations={[second]} /></MemoryRouter>);
    fireEvent.keyDown(document, { key: 'ArrowDown' });
    fireEvent.keyDown(document, { key: 'Enter' });
    expect(open).toHaveBeenCalledWith(second);
  });

  it('dismisses the single action disclosure on Escape and outside click without opening the row', () => {
    const onOpen = vi.fn();
    const row = makeProductConversation('menu');
    const view = render(<MemoryRouter><ConversationList {...defaultProps} productConversations={[row]}
      onProductConversationClick={onOpen} onProductConversationRename={vi.fn()} onProductConversationClose={vi.fn()} /></MemoryRouter>);
    const trigger = view.getByRole('button', { name: 'Actions for conversation Root menu' });
    expect(view.queryByRole('button', { name: 'Rename conversation Root menu' })).toBeNull();
    fireEvent.click(trigger);
    const rename = view.getByRole('button', { name: 'Rename conversation Root menu' });
    rename.focus();
    fireEvent.keyDown(rename, { key: 'Escape' });
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(trigger);
    fireEvent.mouseDown(document.body);
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(onOpen).not.toHaveBeenCalled();
  });

  it.each([
    ['open', { state: 'open', close_action: { availability: 'available' } }, false, true, true, false],
    ['history', { state: 'history' }, true, false, false, true],
    ['closing', { state: 'open', close_action: { availability: 'unavailable', reason: 'active_close_attempt' } }, false, false, true, false],
  ] as const)('preserves %s lifecycle action guards', (_name, lifecycle, history, rename, close, remove) => {
    const row = makeProductConversation('guards', { lifecycle });
    const view = render(<MemoryRouter><ConversationList {...defaultProps} productConversations={history ? [] : [row]}
      archivedProductConversations={history ? [row] : []} showArchived={history}
      onProductConversationRename={vi.fn()} onProductConversationClose={vi.fn()} onProductConversationDelete={vi.fn()} /></MemoryRouter>);
    fireEvent.click(view.getByRole('button', { name: 'Actions for conversation Root guards' }));
    expect(!!view.queryByRole('button', { name: 'Rename conversation Root guards' })).toBe(rename);
    expect(!!view.queryByRole('button', { name: /^Close conversation/ })).toBe(close);
    expect(!!view.queryByRole('button', { name: 'Delete conversation Root guards' })).toBe(remove);
    if (_name === 'closing') expect(view.getByRole('button', { name: /^Close conversation/ })).toBeDisabled();
  });

  it('does not introduce destructive actions when offline callbacks are absent', () => {
    const view = render(<MemoryRouter><ConversationList {...defaultProps} productConversations={[makeProductConversation('offline')]}
      onProductConversationRename={vi.fn()} /></MemoryRouter>);
    fireEvent.click(view.getByRole('button', { name: 'Actions for conversation Root offline' }));
    expect(view.queryByRole('button', { name: /^Close conversation/ })).toBeNull();
    expect(view.queryByRole('button', { name: /^Delete conversation/ })).toBeNull();
  });

  it('exposes keyboard/touch accessible row actions without triggering row navigation', () => {
    const row = makeProductConversation('actions-open', { canonical_root: { transcript_row_id: 'root-actions', slug: 'Action Product', title: 'Action Product' } });
    const onOpen = vi.fn();
    const onRename = vi.fn();
    const onClose = vi.fn();

    const { getByRole } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          productConversations={[row]}
          onProductConversationClick={onOpen}
          onProductConversationRename={onRename}
          onProductConversationClose={onClose}
        />
      </MemoryRouter>,
    );

    fireEvent.click(getByRole('button', { name: 'Actions for conversation Action Product' }));
    const rename = getByRole('button', { name: /Rename conversation Action Product/ });
    const close = getByRole('button', { name: /Close conversation Action Product/ });
    expect(rename).toBeInTheDocument();
    expect(close).toBeInTheDocument();

    rename.focus();
    fireEvent.click(rename);
    fireEvent.click(getByRole('button', { name: 'Actions for conversation Action Product' }));
    fireEvent.click(getByRole('button', { name: /Close conversation Action Product/ }));

    expect(onRename).toHaveBeenCalledWith(row);
    expect(onClose).toHaveBeenCalledWith(row);
    expect(onOpen).not.toHaveBeenCalled();
  });
});


const defaultProps = {
  showArchived: false,
  onToggleArchived: vi.fn(),
  onNewConversation: vi.fn(),
};

describe('ConversationList — product conversations', () => {
  it('renders a single-line row: status dot, one title, no metadata line', () => {
    const open = makeProductConversation('pc-open', {
      canonical_root: { transcript_row_id: 'root-open', slug: 'root-open', title: 'Open Root' },
      presentation: { kind: 'state', display_name: 'Working surface', presentation_mode: 'working' },
      read_state: { state: 'read' },
    });
    const archived = makeProductConversation('pc-archived', {
      lifecycle: { state: 'history' },
      canonical_root: { transcript_row_id: 'root-archived', slug: 'root-archived', title: 'Archived Root' },
      presentation: { kind: 'state', display_name: 'Retained history', presentation_mode: 'done' },
      read_state: { state: 'read' },
    });

    const { rerender, getByText, container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          productConversations={[open]}
          archivedProductConversations={[archived]}
          listDensity="mobile"
        />
      </MemoryRouter>,
    );

    const openRow = container.querySelector('[data-product-conversation-id="pc-open"]') as HTMLElement;
    expect(getByText('Working surface')).toBeInTheDocument();
    expect(openRow.querySelector('.conv-state-dot')?.getAttribute('title')).toBe('Working');
    expect(openRow).not.toHaveTextContent('rel-2024-01-01T00:00:00Z');
    expect(openRow).not.toHaveTextContent('root-open');
    expect(openRow).not.toHaveTextContent('Open Root');
    expect(openRow.querySelectorAll('.conv-item-title')).toHaveLength(1);
    expect(openRow.querySelectorAll('.conv-item-meta')).toHaveLength(0);

    rerender(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          productConversations={[open]}
          archivedProductConversations={[archived]}
          listDensity="mobile"
          showArchived
        />
      </MemoryRouter>,
    );

    const historyRow = container.querySelector('[data-product-conversation-id="pc-archived"]') as HTMLElement;
    expect(getByText('Retained history')).toBeInTheDocument();
    expect(historyRow.querySelector('.conv-state-dot')?.getAttribute('title')).toBe('Completed');
    expect(historyRow).not.toHaveTextContent('Archived Root');
  });

  it('uses server-authoritative Close availability without inferring from lifecycle or presentation', () => {
    const close = vi.fn();
    const blocked = makeProductConversation('pc-blocked', {
      lifecycle: { state: 'open', close_action: { availability: 'unavailable', reason: 'awaiting_task_approval' } },
      presentation: { kind: 'state', display_name: 'Blocked', presentation_mode: 'idle' },
      read_state: { state: 'read' },
    });
    const available = makeProductConversation('pc-available', {
      lifecycle: { state: 'open', close_action: { availability: 'available' } },
      presentation: { kind: 'needs_action', display_name: 'Needs action' },
    });

    const { getByRole } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          productConversations={[blocked, available]}
          onProductConversationClose={close}
        />
      </MemoryRouter>,
    );

    fireEvent.click(getByRole('button', { name: 'Actions for conversation Blocked' }));
    expect(getByRole('button', {
      name: /Close conversation Blocked\. Resolve the pending task approval before closing/,
    })).toBeDisabled();
    fireEvent.click(getByRole('button', { name: 'Actions for conversation Needs action' }));
    fireEvent.click(getByRole('button', { name: 'Close conversation Needs action' }));
    expect(close).toHaveBeenCalledWith(available);
  });

  it('navigates through aggregate canonical routes when no custom click handler is provided', () => {
    function Location() { return <span data-testid="location">{useLocation().pathname}</span>; }
    const row = makeProductConversation('pc-nav', {
      canonical_root: { transcript_row_id: 'root-nav', slug: 'root-nav', title: 'Nav Root' },
    });

    const { getByText, getByTestId } = render(
      <MemoryRouter initialEntries={['/']}>
        <ConversationList
          {...defaultProps}
          productConversations={[row]}
          listDensity="mobile"
        />
        <Location />
      </MemoryRouter>,
    );

    fireEvent.click(getByText('Nav Root'));
    expect(getByTestId('location')).toHaveTextContent('/product-conversations/pc-nav');
  });
});

describe('ConversationList — footer slot', () => {
  it('renders the footer inside the conversation list after the row list', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          listDensity="mobile"
          footer={<footer data-testid="storage-footer">1 conversation</footer>}
        />
      </MemoryRouter>,
    );

    const listRoot = container.querySelector('#conversation-list');
    const rowList = container.querySelector('#conv-list');
    const footerSlot = container.querySelector('.conversation-list-footer');
    const footer = container.querySelector('[data-testid="storage-footer"]');

    expect(listRoot).not.toBeNull();
    expect(rowList).not.toBeNull();
    expect(footerSlot).not.toBeNull();
    expect(footer).not.toBeNull();
    expect(listRoot).toContainElement(footerSlot as HTMLElement);
    expect(footerSlot).toContainElement(footer as HTMLElement);
    expect(rowList?.nextElementSibling).toBe(footerSlot);
  });
});

// SIDE-02: "All" tab has no project labels
//

describe('ConversationList — product row behaviour', () => {
  let originalScrollDescriptor: PropertyDescriptor | undefined;
  let scrollIntoViewSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    originalScrollDescriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
    scrollIntoViewSpy = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, value: scrollIntoViewSpy });
  });

  afterEach(() => {
    if (originalScrollDescriptor) {
      Object.defineProperty(Element.prototype, 'scrollIntoView', originalScrollDescriptor);
    } else {
      delete (Element.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it('scrolls the active product conversation into view by any of its route identities', async () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          productConversations={[makeProductConversation('other'), makeProductConversation('active')]}
          activeSlug="latest-active"
        />
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(scrollIntoViewSpy).toHaveBeenCalledWith({ block: 'nearest', behavior: 'smooth' });
    });
    expect(container.querySelector('[data-id="other"]')!.classList.contains('active')).toBe(false);
  });

  it('arrow-down moves keyboard selection across product rows', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          productConversations={['c1', 'c2', 'c3'].map((id) => makeProductConversation(id))}
        />
      </MemoryRouter>,
    );
    act(() => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' })); });
    act(() => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' })); });
    expect(container.querySelector('[data-id="c2"]')!.classList.contains('keyboard-selected')).toBe(true);
    expect(container.querySelector('[data-id="c1"]')!.classList.contains('keyboard-selected')).toBe(false);
  });

  const pr = {
    number: 42,
    title: 'Ship unread',
    url: 'https://example.invalid/pr/42',
    display_state: 'open',
    base: 'main',
    head: 'unread',
    feedback_status: 'approved',
  } as const;

  it('renders a linked cached PR badge beside the row on desktop', () => {
    const view = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode productConversations={[
          makeProductConversation('with-pr', { cached_pr: pr }),
          makeProductConversation('without-pr'),
        ]} />
      </MemoryRouter>,
    );
    const badge = view.getByRole('link', { name: /PR 42/ });
    expect(badge.getAttribute('href')).toBe(pr.url);
    expect(badge.closest('.conv-item-main')).toBeNull();
    expect(view.container.querySelectorAll('.sidebar-pr-badge')).toHaveLength(1);
  });

  it('renders a non-interactive cached PR badge on mobile rows', () => {
    const view = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" productConversations={[
          makeProductConversation('with-pr', { cached_pr: pr }),
        ]} />
      </MemoryRouter>,
    );
    expect(view.queryByRole('link', { name: /PR 42/ })).toBeNull();
    expect(view.container.querySelector('.product-conversation-list-row__prs .sidebar-pr-badge')).not.toBeNull();
  });
});

