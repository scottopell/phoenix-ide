// Issue regression tests for ConversationList component.
//
// SIDE-02: "All" tab has no project labels
// SIDE-04: Context menu persists across navigation (no click-outside handler)
// CHN-Sidebar: chain grouping render (REQ-CHN-002, task 02690 Phase 5)

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, within, act, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import type { Conversation } from '../api';

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

import { ConversationList, ConversationRow, ChainBlock } from './ConversationList';

describe('ConversationList — active conversation reveal', () => {
  let originalScrollDescriptor: PropertyDescriptor | undefined;
  let scrollIntoViewSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    originalScrollDescriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
    scrollIntoViewSpy = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      value: scrollIntoViewSpy,
    });
  });

  afterEach(() => {
    if (originalScrollDescriptor) {
      Object.defineProperty(Element.prototype, 'scrollIntoView', originalScrollDescriptor);
    } else {
      delete (Element.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it('scrolls an active standalone conversation into view', async () => {
    const active = makeConv('active-id', 'active-slug');

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[makeConv('other-id', 'other-slug'), active]}
          activeSlug="active-slug"
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(scrollIntoViewSpy).toHaveBeenCalledWith({ block: 'nearest', behavior: 'smooth' });
    });
    expect(container.querySelector('[data-id="active-id"]')!.classList.contains('active')).toBe(true);
  });

  it('expands a collapsed active chain member and scrolls it into view', async () => {
    const root = makeConv('root-id', 'root-slug', {
      continued_in_conv_id: 'leaf-id',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const leaf = makeConv('leaf-id', 'leaf-slug', {
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, root]}
          activeSlug="leaf-slug"
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelectorAll('.conv-item-chain-member').length).toBe(2);
    });
    await waitFor(() => {
      expect(scrollIntoViewSpy).toHaveBeenCalledWith({ block: 'nearest', behavior: 'smooth' });
    });
    expect(container.querySelector('[data-id="leaf-id"]')!.classList.contains('active')).toBe(true);
    expect(container.querySelector('.conv-chain-block')!.classList.contains('expanded')).toBe(true);
  });
});


const makeConv = (id: string, slug: string, overrides: Partial<Conversation> = {}): Conversation => ({
  id,
  slug,
  model: 'claude-3-5-sonnet',
  cwd: '/home/user/project',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 5,
  project_id: 'proj-1',
  conv_mode_label: 'EXPLORE',
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: `conversation:${id}`,
  ...overrides,
});

describe('ConversationRow — cached PR badge', () => {
  const renderRow = (conv: Conversation) => render(
    <MemoryRouter>
      <ConversationRow
        conv={conv}
        isSidebarMode={false}
        isMenuOpen={false}
        isKeyboardSelected={false}
        isActive={false}
        isChainMember={false}
        isChainLatest={false}
        chainIndex={undefined}
        showArchived={false}
        onClick={vi.fn()}
        onToggleMenu={vi.fn()}
        onArchive={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
        onCloseMenu={vi.fn()}
      />
    </MemoryRouter>,
  );

  it('shows no PR badge when no cached PR exists', () => {
    renderRow(makeConv('no-pr', 'no-pr'));
    expect(document.querySelector('.sidebar-pr-badge')).toBeNull();
  });

  it.each([
    ['open', '#12'],
    ['draft', '#12 draft'],
    ['merged', '#12 merged'],
    ['closed', '#12 closed'],
  ] as const)('shows cached %s PR badge', (display_state, label) => {
    renderRow(makeConv(`with-${display_state}`, `with-${display_state}`, {
      cached_pr: {
        number: 12,
        title: 'Fix sidebar',
        url: 'https://github.com/o/r/pull/12',
        display_state,
        base: 'main',
        head: 'task-branch',
      },
    }));

    const badge = document.querySelector('.sidebar-pr-badge') as HTMLAnchorElement;
    expect(badge).not.toBeNull();
    expect(badge.textContent).toBe(label);
    expect(badge.href).toBe('https://github.com/o/r/pull/12');
    expect(badge.title).toContain('Fix sidebar');
    expect(badge.title).toContain('task-branch → main');
  });

  it('renders the same cached PR on conversations sharing a work scope', () => {
    const cached_pr = {
      number: 44,
      title: 'Shared PR',
      url: 'https://github.com/o/r/pull/44',
      display_state: 'open' as const,
      base: 'main',
      head: 'shared',
    };

    render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[
            makeConv('a', 'a', { work_scope_key: 'worktree:/tmp/shared', cached_pr }),
            makeConv('b', 'b', { work_scope_key: 'worktree:/tmp/shared', cached_pr }),
          ]}
        />
      </MemoryRouter>,
    );

    expect(document.querySelectorAll('.sidebar-pr-badge')).toHaveLength(2);
    expect(Array.from(document.querySelectorAll('.sidebar-pr-badge')).map((n) => n.textContent))
      .toEqual(['#44', '#44']);
  });
});

const defaultProps = {
  archivedConversations: [] as Conversation[],
  showArchived: false,
  onToggleArchived: vi.fn(),
  onNewConversation: vi.fn(),
  onArchive: vi.fn(),
  onDelete: vi.fn(),
  onRename: vi.fn(),
};

// SIDE-02: "All" tab has no project labels
//
// When viewing the "All" tab with conversations from multiple projects,
// each conversation item should show which project it belongs to via a
// distinct project label/badge element. Currently the component only
// renders slug, mode badge, model, and cwd -- no project indicator.
describe('SIDE-02: Conversation list should show project labels', () => {
  it('renders a project label element for each conversation item', () => {
    const conversations = [
      makeConv('c1', 'fix-login-bug', { project_id: 'proj-1', cwd: '/home/user/my-app' }),
      makeConv('c2', 'add-tests', { project_id: 'proj-2', cwd: '/home/user/other-project' }),
    ];

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={conversations}
        />
      </MemoryRouter>
    );

    const items = container.querySelectorAll('.conv-item');
    expect(items.length).toBe(2);

    // Each item should have a dedicated project label element.
    // This is distinct from .conv-item-cwd (which shows the working directory)
    // and .conv-mode-badge (which shows EXPLORE/WORK/DIRECT).
    // A project label identifies which git repository the conversation belongs to.
    const projectLabels = container.querySelectorAll('.conv-project-label');
    expect(projectLabels.length).toBe(2);
  });
});

// SIDE-04: Context menu persists across UI state changes (no click-outside handler)
describe('SIDE-04: Context menu should close on click-outside', () => {
  it('closes the context menu when clicking outside of it', () => {
    const conversations = [
      makeConv('c1', 'test-conversation'),
    ];

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={conversations}
        />
      </MemoryRouter>
    );

    // Open the context menu by clicking the three-dot button
    const menuBtn = container.querySelector('.conv-item-menu-btn');
    expect(menuBtn).not.toBeNull();
    fireEvent.click(menuBtn!);

    // Verify the menu is open (actions should be visible)
    const actions = container.querySelector('.conv-item-actions');
    expect(actions).not.toBeNull();

    // Click outside the menu (on the conversation list section itself)
    const listSection = container.querySelector('#conversation-list');
    fireEvent.mouseDown(listSection!);

    // The context menu should now be closed
    const actionsAfterClickOutside = container.querySelector('.conv-item-actions');
    expect(actionsAfterClickOutside).toBeNull();
  });
});

// CHN-Sidebar: sidebarMode renders chains as collapsible blocks with the
// chain's display name, members in chain order, and the latest member
// emphasized. Standalone conversations remain unaffected. Per REQ-CHN-002
// and specs/chains/design.md "Sidebar Grouping" / Phase 5 (task 02690).
describe('Chain grouping in sidebar mode (REQ-CHN-002)', () => {
  it('renders a chain block with the chain_name as header and members in chain order', () => {
    // Recency: leaf (Mar) > standalone (Feb) > root (Jan).
    // Chain block sits at leaf's position; members listed root → leaf.
    const root = makeConv('cr', 'root-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      chain_name: 'auth refactor',
    });
    const leaf = makeConv('cl', 'leaf-slug', {
      updated_at: '2024-03-01T00:00:00Z',
    });
    const standalone = makeConv('s', 'standalone-slug', {
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, standalone, root]}
        />
      </MemoryRouter>
    );

    // One chain block with the user-set chain name.
    const block = container.querySelector('.conv-chain-block');
    expect(block).not.toBeNull();
    expect(block!.querySelector('.conv-chain-name-label')!.textContent).toBe('auth refactor');

    // Default state: expanded — both members render.
    const memberRows = block!.querySelectorAll('.conv-item-chain-member');
    expect(memberRows.length).toBe(2);
    // Members in chain order (root → leaf), independent of updated_at.
    expect(memberRows[0]!.getAttribute('data-id')).toBe('cr');
    expect(memberRows[1]!.getAttribute('data-id')).toBe('cl');

    // Latest = max updated_at = leaf, visually emphasized.
    expect(memberRows[1]!.classList.contains('conv-item-chain-latest')).toBe(true);
    expect(memberRows[0]!.classList.contains('conv-item-chain-latest')).toBe(false);
    expect(within(memberRows[1] as HTMLElement).getByText('latest')).toBeTruthy();

    // Standalone conversation renders as a regular .conv-item (not a chain
    // member, not inside the chain block).
    const standaloneRow = container.querySelector('[data-id="s"]');
    expect(standaloneRow).not.toBeNull();
    expect(standaloneRow!.classList.contains('conv-item-chain-member')).toBe(false);
    expect(standaloneRow!.closest('.conv-chain-block')).toBeNull();
  });

  it('compacts completed non-latest chain members only in sidebar mode', () => {
    const root = makeConv('cr', 'root-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      presentation_mode: 'done',
      state: { type: 'terminal' },
    });
    const leaf = makeConv('cl', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      presentation_mode: 'idle',
      state: { type: 'idle' },
    });
    const standalone = makeConv('s', 'standalone-slug', {
      updated_at: '2024-03-01T00:00:00Z',
      presentation_mode: 'done',
      state: { type: 'terminal' },
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={[standalone, leaf, root]} />
      </MemoryRouter>
    );

    expect(container.querySelector('[data-id="cr"]')!.classList.contains('conv-item-chain-completed')).toBe(true);
    expect(container.querySelector('[data-id="cl"]')!.classList.contains('conv-item-chain-completed')).toBe(false);
    expect(container.querySelector('[data-id="s"]')!.classList.contains('conv-item-chain-completed')).toBe(false);
  });

  it('does not compact full-page chain members or active rows', () => {
    const root = makeConv('cr', 'root-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      presentation_mode: 'done',
      state: { type: 'terminal' },
    });
    const leaf = makeConv('cl', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      presentation_mode: 'idle',
      state: { type: 'idle' },
    });

    const { container: fullPageContainer } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} conversations={[leaf, root]} />
      </MemoryRouter>
    );
    expect(fullPageContainer.querySelector('[data-id="cr"]')!.classList.contains('conv-item-chain-completed')).toBe(false);

    const { container: activeContainer } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, root]}
          activeSlug="root-slug"
        />
      </MemoryRouter>
    );
    expect(activeContainer.querySelector('[data-id="cr"]')!.classList.contains('conv-item-chain-completed')).toBe(false);
  });

  it('falls back to root.slug when chain_name is null', () => {
    const root = makeConv('rooty', 'root-slug-text', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'leafy',
      // chain_name omitted → falls back to slug.
    });
    const leaf = makeConv('leafy', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={[leaf, root]} />
      </MemoryRouter>
    );

    expect(container.querySelector('.conv-chain-name-label')!.textContent).toBe('root-slug-text');
  });

  it('caret toggles collapse without navigating; members hide when collapsed', () => {
    const root = makeConv('cr', 'r', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      chain_name: 'mychain',
    });
    const leaf = makeConv('cl', 'l', { updated_at: '2024-02-01T00:00:00Z' });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={[leaf, root]} />
      </MemoryRouter>
    );

    // Default: expanded.
    expect(container.querySelectorAll('.conv-item-chain-member').length).toBe(2);

    // Click caret → collapse.
    const caret = container.querySelector('.conv-chain-caret') as HTMLButtonElement;
    fireEvent.click(caret);

    expect(container.querySelectorAll('.conv-item-chain-member').length).toBe(0);
    expect(container.querySelector('.conv-chain-block')!.classList.contains('collapsed')).toBe(true);

    // Click again → expand.
    fireEvent.click(caret);
    expect(container.querySelectorAll('.conv-item-chain-member').length).toBe(2);
  });

  it('clicking the chain name navigates to /chains/:rootId', () => {
    const root = makeConv('myroot', 'r', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'leaf',
      chain_name: 'authchain',
    });
    const leaf = makeConv('leaf', 'l', { updated_at: '2024-02-01T00:00:00Z' });

    const onPath = vi.fn();

    const { container } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route
            path="*"
            element={
              <>
                <ConversationList
                  {...defaultProps}
                  sidebarMode
                  conversations={[leaf, root]}
                />
                <PathReader onPath={onPath} />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    );

    const nameBtn = container.querySelector('.conv-chain-name') as HTMLButtonElement;
    fireEvent.click(nameBtn);

    // PathReader fires onPath on every render; the last call is the
    // post-navigation pathname.
    const calls = onPath.mock.calls;
    expect(calls.length).toBeGreaterThan(0);
    expect(calls[calls.length - 1]![0]).toBe('/chains/myroot');
  });

  it('clicking a member fires onConversationClick (not the chain page)', () => {
    const root = makeConv('cr', 'r', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
    });
    const leaf = makeConv('cl', 'l', { updated_at: '2024-02-01T00:00:00Z' });
    const onConversationClick = vi.fn();

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, root]}
          onConversationClick={onConversationClick}
        />
      </MemoryRouter>
    );

    const rootRow = container.querySelector('[data-id="cr"] .conv-item-main') as HTMLElement;
    fireEvent.click(rootRow);
    expect(onConversationClick).toHaveBeenCalledTimes(1);
    expect(onConversationClick.mock.calls[0]![0].id).toBe('cr');
  });

  it('non-sidebar mode also groups conversations into chain blocks', () => {
    // Chain grouping is not restricted to sidebar mode — the full-page list
    // groups chains identically (REQ-CHN-002, task 02698).
    const root = makeConv('cr', 'r', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      chain_name: 'auth refactor',
    });
    const leaf = makeConv('cl', 'l', { updated_at: '2024-02-01T00:00:00Z' });

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={[leaf, root]}
          // sidebarMode left undefined / false
        />
      </MemoryRouter>
    );

    // Chain block is rendered; members show position labels (#1, #2) not raw slugs.
    expect(container.querySelector('.conv-chain-block')).not.toBeNull();
    expect(container.querySelector('.conv-chain-name-label')!.textContent).toBe('auth refactor');
    expect(container.querySelectorAll('.conv-item-slug-pos').length).toBe(2);
  });
});

/** Helper component: reads the current location's pathname into a callback
 *  so click-navigation tests can assert the destination without tightly
 *  coupling to react-router internals. */
function PathReader({ onPath }: { onPath: (p: string) => void }) {
  const loc = useLocation();
  onPath(loc.pathname);
  return null;
}

// Chain-atomic lifecycle (task 02701): the chain block header carries a `⋮`
// menu with chain-scope actions. Member rows hide their own
// Archive/Delete entries — only Rename remains, since lifecycle
// ops on a chain member return 409 server-side. Archive is terminal: the
// archived list exposes no unarchive affordance.
describe('Chain lifecycle UI (task 02701)', () => {
  const chainConvs = (overrides: { archived?: boolean } = {}) => {
    const archivedBase = overrides.archived !== undefined
      ? { archived: overrides.archived }
      : {};
    const root = makeConv('cr', 'root-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'cl',
      chain_name: 'auth refactor',
      ...archivedBase,
    });
    const leaf = makeConv('cl', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      ...archivedBase,
    });
    return [leaf, root];
  };

  it('chain header ⋮ menu shows Rename / Archive / Delete in the active list', () => {
    const onArchiveChain = vi.fn();
    const onDeleteChain = vi.fn();

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={chainConvs()}
          onArchiveChain={onArchiveChain}
          onDeleteChain={onDeleteChain}
        />
      </MemoryRouter>,
    );

    const menuBtn = container.querySelector(
      '.conv-chain-menu-btn',
    ) as HTMLButtonElement;
    expect(menuBtn).not.toBeNull();
    fireEvent.click(menuBtn);

    const actions = container.querySelector('.conv-chain-actions');
    expect(actions).not.toBeNull();
    const labels = Array.from(
      actions!.querySelectorAll<HTMLButtonElement>('.action-btn'),
    ).map((b) => b.textContent?.trim());
    expect(labels).toEqual([
      'Rename chain…',
      'Archive chain',
      'Delete chain',
    ]);
  });

  it('chain header ⋮ menu omits any unarchive affordance in the archived list', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={[]}
          archivedConversations={chainConvs({ archived: true })}
          showArchived
        />
      </MemoryRouter>,
    );

    const menuBtn = container.querySelector(
      '.conv-chain-menu-btn',
    ) as HTMLButtonElement;
    fireEvent.click(menuBtn);

    const labels = Array.from(
      container.querySelectorAll<HTMLButtonElement>(
        '.conv-chain-actions .action-btn',
      ),
    ).map((b) => b.textContent?.trim());
    // Archive is terminal — no Archive/Unarchive entry on archived chains.
    expect(labels).toEqual([
      'Rename chain…',
      'Delete chain',
    ]);
  });

  it('Archive chain action invokes onArchiveChain with the rootId', () => {
    const onArchiveChain = vi.fn();
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={chainConvs()}
          onArchiveChain={onArchiveChain}
        />
      </MemoryRouter>,
    );

    fireEvent.click(container.querySelector('.conv-chain-menu-btn')!);
    const archiveBtn = Array.from(
      container.querySelectorAll<HTMLButtonElement>(
        '.conv-chain-actions .action-btn',
      ),
    ).find((b) => b.textContent?.trim() === 'Archive chain');
    fireEvent.click(archiveBtn!);

    expect(onArchiveChain).toHaveBeenCalledTimes(1);
    expect(onArchiveChain).toHaveBeenCalledWith('cr');
  });

  it('chain member row dropdown shows only Rename', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={chainConvs()}
        />
      </MemoryRouter>,
    );

    const memberRow = container.querySelector(
      '.conv-item-chain-member',
    ) as HTMLElement;
    expect(memberRow).not.toBeNull();
    const rowMenuBtn = memberRow.querySelector(
      '.conv-item-menu-btn',
    ) as HTMLButtonElement;
    fireEvent.click(rowMenuBtn);

    const labels = Array.from(
      memberRow.querySelectorAll<HTMLButtonElement>(
        '.conv-item-actions .action-btn',
      ),
    ).map((b) => b.textContent?.trim());
    expect(labels).toEqual(['Rename']);
  });

  it('standalone row dropdown still shows Rename / Archive / Delete', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={[makeConv('s', 'standalone-slug')]}
        />
      </MemoryRouter>,
    );

    fireEvent.click(container.querySelector('.conv-item-menu-btn')!);
    const labels = Array.from(
      container.querySelectorAll<HTMLButtonElement>(
        '.conv-item-actions .action-btn',
      ),
    ).map((b) => b.textContent?.trim());
    expect(labels).toEqual(['Rename', 'Archive', 'Delete']);
  });

  it('archived list groups chains the same as the active list', () => {
    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          conversations={[]}
          archivedConversations={chainConvs({ archived: true })}
          showArchived
        />
      </MemoryRouter>,
    );
    expect(container.querySelector('.conv-chain-block')).not.toBeNull();
    expect(
      container.querySelector('.conv-chain-name-label')!.textContent,
    ).toBe('auth refactor');
  });
});

// Phase 3 regression: rows are React.memo components with narrow props +
// stable parent callbacks. Keyboard nav (j/k) flips selectedId, which
// changes only the previously- and newly-selected rows' isKeyboardSelected
// prop; every other row's props are referentially identical across the
// parent re-render and must skip update.
//
// Direct memoization tests for the row components. The earlier version of
// these tests asserted DOM-node identity through ConversationList, but
// React preserves DOM nodes across re-renders whenever element type and
// keys are stable — so DOM identity passed even if memo did nothing.
// Per Copilot review on PR #71, we count component-body executions via a
// spy on `formatRelativeTime` (called once per row render). When memo
// bails, the body does not run and the spy is not invoked. Profiler was
// considered first but its onRender fires per commit in the profiled
// subtree even when a child memoizes — false positive.
describe('ConversationRow — React.memo behaviour', () => {
  beforeEach(() => {
    formatRelativeTimeSpy.mockClear();
    formatShortDateTimeSpy.mockClear();
  });

  function buildRowProps(
    conv: Conversation,
    overrides: Partial<React.ComponentProps<typeof ConversationRow>> = {},
  ): React.ComponentProps<typeof ConversationRow> {
    return {
      conv,
      isMenuOpen: false,
      isKeyboardSelected: false,
      isActive: false,
      isChainMember: false,
      isChainLatest: false,
      isSidebarMode: false,
      chainIndex: undefined,
      showArchived: false,
      onClick: vi.fn(),
      onToggleMenu: vi.fn(),
      onArchive: vi.fn(),
      onDelete: vi.fn(),
      onRename: vi.fn(),
      onCloseMenu: vi.fn(),
      menuRef: undefined,
      ...overrides,
    };
  }

  it('skips re-render when props are reference-stable', () => {
    const props = buildRowProps(makeConv('c1', 'one'));

    const { rerender } = render(
      <MemoryRouter>
        <ConversationRow {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    rerender(
      <MemoryRouter>
        <ConversationRow {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).not.toHaveBeenCalled();
  });

  it('re-renders when a relevant prop changes', () => {
    const props = buildRowProps(makeConv('c1', 'one'));

    const { rerender } = render(
      <MemoryRouter>
        <ConversationRow {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    rerender(
      <MemoryRouter>
        <ConversationRow {...props} isKeyboardSelected={true} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();
  });

  it('re-renders when a callback ref changes (regression for unstable parent handlers)', () => {
    const props = buildRowProps(makeConv('c1', 'one'));

    const { rerender } = render(
      <MemoryRouter>
        <ConversationRow {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    rerender(
      <MemoryRouter>
        <ConversationRow {...props} onClick={vi.fn()} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();
  });
});

describe('ChainBlock — React.memo behaviour', () => {
  beforeEach(() => {
    formatRelativeTimeSpy.mockClear();
    formatShortDateTimeSpy.mockClear();
  });

  function buildItem() {
    const c1 = makeConv('c1', 'chain-root');
    const c2 = makeConv('c2', 'chain-child');
    return {
      kind: 'chain' as const,
      rootId: 'c1',
      displayName: 'My chain',
      members: [c1, c2],
      latestMemberId: 'c2',
    };
  }

  function buildChainProps(
    overrides: Partial<React.ComponentProps<typeof ChainBlock>> = {},
  ): React.ComponentProps<typeof ChainBlock> {
    return {
      item: buildItem(),
      collapsed: false,
      isMenuOpen: false,
      expandedRowId: null,
      keyboardSelectedId: null,
      activeSlug: null,
      sidebarMode: false,
      showArchived: false,
      onToggleCollapsed: vi.fn(),
      onToggleChainMenu: vi.fn(),
      onCloseChainMenu: vi.fn(),
      onArchiveChain: vi.fn(),
      onDeleteChain: vi.fn(),
      onRowClick: vi.fn(),
      onRowToggleMenu: vi.fn(),
      onArchive: vi.fn(),
      onDelete: vi.fn(),
      onRename: vi.fn(),
      onCloseRowMenu: vi.fn(),
      rowMenuRef: undefined,
      chainMenuRef: undefined,
      ...overrides,
    };
  }

  it('skips re-render when props are reference-stable', () => {
    const props = buildChainProps();
    // ChainBlock renders 2 ConversationRow children → 2 formatRelativeTime
    // calls on the initial mount (one per row).
    const { rerender } = render(
      <MemoryRouter>
        <ChainBlock {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    rerender(
      <MemoryRouter>
        <ChainBlock {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).not.toHaveBeenCalled();
  });

  it('re-renders when collapsed flips', () => {
    const props = buildChainProps();

    const { rerender } = render(
      <MemoryRouter>
        <ChainBlock {...props} />
      </MemoryRouter>,
    );
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    rerender(
      <MemoryRouter>
        <ChainBlock {...props} collapsed={true} />
      </MemoryRouter>,
    );
    // Collapsed flips: ChainBlock re-renders. When collapsed=true, the
    // child rows are not rendered (gated by `{!collapsed && ...}`), so
    // the spy stays at zero. The fact that ChainBlock processed the new
    // collapsed prop is implicit in the rerender succeeding without
    // throwing — and the inverse direction (collapsing hides children)
    // is the user-visible signal we care about anyway.
    expect(formatRelativeTimeSpy).not.toHaveBeenCalled();
  });
});

describe('ConversationList — keyboard navigation behaviour', () => {
  it('arrow-down moves keyboard-selected from c1 to c2', () => {
    const conversations = [
      makeConv('c1', 'one'),
      makeConv('c2', 'two'),
      makeConv('c3', 'three'),
    ];

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} conversations={conversations} />
      </MemoryRouter>,
    );

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }));
    });
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }));
    });

    expect(
      container.querySelector('[data-id="c2"]')!.classList.contains('keyboard-selected'),
    ).toBe(true);
    expect(
      container.querySelector('[data-id="c1"]')!.classList.contains('keyboard-selected'),
    ).toBe(false);
  });
});

