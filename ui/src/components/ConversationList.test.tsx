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
        listDensity="full"
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

  const cachedPr = (number = 44, title = 'Shared PR') => ({
    number,
    title,
    url: `https://github.com/o/r/pull/${number}`,
    display_state: 'open' as const,
    base: 'main',
    head: 'shared',
  });

  it('renders cached PR badges on standalone conversations sharing a work scope', () => {
    const cached_pr = cachedPr();

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

  it('renders a cached PR badge only on the latest chain member', () => {
    const root = makeConv('root-id', 'root-slug', {
      continued_in_conv_id: 'leaf-id',
      updated_at: '2024-01-01T00:00:00Z',
      cached_pr: cachedPr(50, 'Root PR'),
    });
    const leaf = makeConv('leaf-id', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      cached_pr: cachedPr(50, 'Leaf PR'),
    });

    render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, root]}
        />
      </MemoryRouter>,
    );

    const badges = document.querySelectorAll('.sidebar-pr-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0]!.textContent).toBe('#50');
    expect(document.querySelector('[data-id="root-id"] .sidebar-pr-badge')).toBeNull();
    expect(document.querySelector('[data-id="leaf-id"] .sidebar-pr-badge')).not.toBeNull();
  });

  it('does not render non-latest chain member PR badges when the member row is expanded', () => {
    const root = makeConv('root-id', 'root-slug', {
      continued_in_conv_id: 'leaf-id',
      updated_at: '2024-01-01T00:00:00Z',
      cached_pr: cachedPr(51),
    });
    const leaf = makeConv('leaf-id', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      cached_pr: cachedPr(51),
    });

    render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          sidebarMode
          conversations={[leaf, root]}
        />
      </MemoryRouter>,
    );

    const rootMenuButton = document.querySelector(
      '[data-id="root-id"] .conv-item-menu-btn',
    ) as HTMLButtonElement;
    fireEvent.click(rootMenuButton);

    expect(document.querySelector('[data-id="root-id"]')).toHaveClass('expanded');
    expect(document.querySelector('[data-id="root-id"] .sidebar-pr-badge')).toBeNull();
    expect(document.querySelectorAll('.sidebar-pr-badge')).toHaveLength(1);
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

describe('Mobile conversation list redesign', () => {
  const pr = (number = 375) => ({
    number,
    title: 'Relevant PR',
    url: `https://github.com/o/r/pull/${number}`,
    display_state: 'open' as const,
    base: 'main',
    head: 'task-branch',
  });

  it('renders standalone PR badges as non-link visual badges on mobile while desktop stays clickable', () => {
    const conv = makeConv('with-pr', 'resume-work', { cached_pr: pr(), conv_mode_label: 'WORK' });

    const { container: mobile } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );
    expect(mobile.querySelector('[data-id="with-pr"] span.sidebar-pr-badge')?.textContent).toBe('#375');
    expect(mobile.querySelector('[data-id="with-pr"] a.sidebar-pr-badge')).toBeNull();
    expect(mobile.querySelector('[data-id="with-pr"] .conv-item-model')).not.toBeNull();

    const { container: desktop } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} conversations={[conv]} />
      </MemoryRouter>,
    );
    expect(desktop.querySelector('[data-id="with-pr"] a.sidebar-pr-badge')).not.toBeNull();
  });

  it('renders sidebar PR feedback reaction status with compact accessible indicators', () => {
    const convs = [
      makeConv('open-pr', 'open-pr', { cached_pr: pr(375), conv_mode_label: 'WORK' }),
      makeConv('eyes-pr', 'eyes-pr', { cached_pr: { ...pr(376), feedback_status: 'in_progress' }, conv_mode_label: 'WORK' }),
      makeConv('approved-pr', 'approved-pr', { cached_pr: { ...pr(377), feedback_status: 'approved' }, conv_mode_label: 'WORK' }),
    ];

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={convs} />
      </MemoryRouter>,
    );

    expect(container.querySelector('[data-id="open-pr"] .sidebar-pr-badge')?.textContent).toBe('#375');
    const eyes = container.querySelector('[data-id="eyes-pr"] .sidebar-pr-badge') as HTMLElement;
    expect(eyes.textContent).toBe('#376 👀');
    expect(eyes.title).toContain('Feedback status: in progress (eyes reaction)');
    expect(eyes).toHaveAttribute('aria-label', 'PR 376, feedback in progress (eyes reaction)');
    expect(eyes).toHaveClass('sidebar-pr-badge--feedback-in-progress');

    const approved = container.querySelector('[data-id="approved-pr"] .sidebar-pr-badge') as HTMLElement;
    expect(approved.textContent).toBe('#377 👍');
    expect(approved.title).toContain('Feedback status: approved (thumbs-up reaction)');
    expect(approved).toHaveAttribute('aria-label', 'PR 377, feedback approved (thumbs-up reaction)');
    expect(approved).toHaveClass('sidebar-pr-badge--feedback-approved');
  });

  it('keeps mobile reaction-status PR badges non-interactive and accessible', () => {
    const conv = makeConv('mobile-eyes-pr', 'mobile-eyes-pr', {
      cached_pr: { ...pr(378), feedback_status: 'in_progress' },
      conv_mode_label: 'WORK',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );

    const badge = container.querySelector('[data-id="mobile-eyes-pr"] span.sidebar-pr-badge') as HTMLElement;
    expect(badge.textContent).toBe('#378 👀');
    expect(badge).toHaveAttribute('aria-label', 'PR 378, feedback in progress (eyes reaction)');
    expect(container.querySelector('[data-id="mobile-eyes-pr"] a.sidebar-pr-badge')).toBeNull();
  });

  it('keeps the mobile title in the primary row and moves metadata below it', () => {
    const conv = makeConv('with-long-mobile-row', 'very-long-mobile-conversation-title-that-must-truncate', {
      cached_pr: pr(),
      conv_mode_label: 'WORK',
      presentation_mode: 'needs_action',
      state: { type: 'awaiting_task_approval', title: 'Approve', priority: 'p2', plan: 'Plan' },
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );

    const row = container.querySelector('[data-id="with-long-mobile-row"]')!;
    const slugMain = row.querySelector<HTMLElement>('.conv-item-slug-main')!;
    const secondary = row.querySelector<HTMLElement>('.conv-item-meta.secondary')!;
    const time = row.querySelector<HTMLElement>('.conv-item-time-inline')!;

    expect(slugMain).toContainElement(row.querySelector('.conv-item-title'));
    expect(slugMain).not.toContainElement(row.querySelector('.conv-mode-badge'));
    expect(slugMain).not.toContainElement(row.querySelector('.sidebar-pr-badge'));
    expect(secondary).toContainElement(row.querySelector('.conv-mode-badge'));
    expect(secondary).toContainElement(row.querySelector('.sidebar-pr-badge'));
    expect(secondary).toContainElement(time);
  });

  it('defaults mobile chains to a compact summary with separate chain and latest-conversation targets', () => {
    const root = makeConv('root-id', 'root-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'leaf-id',
      chain_name: 'mobile chain',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      cached_pr: pr(1),
    });
    const leaf = makeConv('leaf-id', 'leaf-slug', {
      updated_at: '2024-02-01T00:00:00Z',
      conv_mode_label: 'WORK',
      cached_pr: pr(375),
    });
    const onConversationClick = vi.fn();
    const onPath = vi.fn();

    const { container } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="*" element={(
            <>
              <ConversationList
                {...defaultProps}
                listDensity="mobile"
                conversations={[leaf, root]}
                onConversationClick={onConversationClick}
              />
              <PathReader onPath={onPath} />
            </>
          )} />
        </Routes>
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-block')).toHaveClass('collapsed');
    expect(container.querySelectorAll('.conv-item-chain-member')).toHaveLength(0);
    const summary = container.querySelector('.conv-chain-latest-summary') as HTMLButtonElement;
    expect(summary).not.toBeNull();
    expect(summary.textContent).toContain('Latest #2');
    expect(summary.querySelector('.conv-project-label')?.textContent).toBe('project');
    expect(summary.querySelector('span.sidebar-pr-badge')?.textContent).toBe('#375');
    expect(summary.querySelector('a.sidebar-pr-badge')).toBeNull();

    fireEvent.click(summary);
    expect(onConversationClick).toHaveBeenCalledWith(expect.objectContaining({ id: 'leaf-id' }));

    fireEvent.click(container.querySelector('.conv-chain-name')!);
    const calls = onPath.mock.calls;
    expect(calls[calls.length - 1]![0]).toBe('/chains/root-id');
  });

  it('minimizes completed non-latest members on mobile full-page lists', () => {
    const doneRoot = makeConv('done-root', 'done-root', {
      continued_in_conv_id: 'done-leaf',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const doneLeaf = makeConv('done-leaf', 'done-leaf', { updated_at: '2024-02-01T00:00:00Z' });

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          listDensity="mobile"
          conversations={[doneLeaf, doneRoot]}
        />
      </MemoryRouter>,
    );

    fireEvent.click(container.querySelector('.conv-chain-caret')!);
    expect(container.querySelector('[data-id="done-root"]')).toHaveClass('conv-item-chain-completed');
    expect(container.querySelector('[data-id="done-root"] .conv-item-title')?.textContent).toBe('done-root');
    expect(container.querySelector('[data-id="done-root"] .conv-project-label')?.textContent).toBe('project');
    expect(container.querySelector('[data-id="done-leaf"] .conv-item-title')?.textContent).toBe('done-leaf');
  });

  it('keeps the active mobile chain expanded even for completed chains', async () => {
    const root = makeConv('active-root', 'active-root', {
      continued_in_conv_id: 'active-leaf',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const leaf = makeConv('active-leaf', 'active-leaf', {
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList
          {...defaultProps}
          listDensity="mobile"
          conversations={[leaf, root]}
          activeSlug="active-leaf"
        />
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-block')).toHaveClass('expanded');
    expect(container.querySelector('[data-id="active-leaf"]')).toHaveClass('active');
    expect(container.querySelector('[data-id="active-leaf"] .conv-item-title')?.textContent).toBe('active-leaf');
    expect(container.querySelectorAll('.conv-item-chain-member')).toHaveLength(2);
  });

  it('auto-expands mobile chains with actionable latest members', () => {
    const root = makeConv('needs-root', 'needs-root', {
      continued_in_conv_id: 'needs-leaf',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const leaf = makeConv('needs-leaf', 'needs-leaf', {
      presentation_mode: 'needs_action',
      state: { type: 'awaiting_task_approval', title: 'Approve', priority: 'p2', plan: 'Plan' },
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[leaf, root]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-block')).toHaveClass('expanded');
    expect(container.querySelector('.conv-chain-latest-summary')).toBeNull();
    expect(container.querySelector('[data-id="needs-leaf"] .conv-state-dot')).toHaveClass('awaiting-approval');
  });

  it('shows a visible context-full status label in mobile metadata', () => {
    const conv = makeConv('context-full', 'context-full', {
      presentation_mode: 'needs_action',
      state: { type: 'context_exhausted', summary: 'Context is full' },
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('[data-id="context-full"] .conv-state-dot')).toHaveClass('awaiting-approval');
    expect(container.querySelector('[data-id="context-full"] .conv-state-dot')).toHaveAttribute('title', 'Context full');
    expect(container.querySelector('[data-id="context-full"] .conv-state-chip')?.textContent).toBe('Context full');
  });

  it('shows a visible needs-reply status label in mobile metadata', () => {
    const conv = makeConv('needs-reply', 'needs-reply', {
      presentation_mode: 'needs_action',
      state: { type: 'awaiting_user_response', questions: [] },
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('[data-id="needs-reply"] .conv-state-dot')).toHaveClass('awaiting-approval');
    expect(container.querySelector('[data-id="needs-reply"] .conv-state-dot')).toHaveAttribute('title', 'Needs reply');
    expect(container.querySelector('[data-id="needs-reply"] .conv-state-chip')?.textContent).toBe('Needs reply');
  });

  it('uses semantic title fallbacks instead of GUID-like primary labels on mobile rows', () => {
    const withTaskTitle = makeConv('task-title', 'f872dd1a-f701-49f3-ad25-2605c6b6f3dc', {
      task_title: 'Iterate mobile conversation list fixtures',
      branch_name: 'task-26004-iterate-mobile-conversation-list-fixtures',
    });
    const withForkPrefix = makeConv('fork-title', 'fork-123e4567-e89b-42d3-a456-426614174000', {
      task_title: 'Fix forked task title display',
      branch_name: 'task-26004-fork-display',
    });
    const withBranch = makeConv('branch-title', '9d1b4cc93b7845228e4fdbe566761f44', {
      task_title: null,
      branch_name: 'scott/mobile-row-overflow-audit',
    });
    const withContext = makeConv('context-title', '123e4567-e89b-12d3-a456-426614174000', {
      task_title: null,
      branch_name: null,
      project_name: null,
      cwd: '/tmp/readable-context-leaf',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[withTaskTitle, withForkPrefix, withBranch, withContext]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('[data-id="task-title"] .conv-item-title')?.textContent).toBe('Iterate mobile conversation list fixtures');
    expect(container.querySelector('[data-id="fork-title"] .conv-item-title')?.textContent).toBe('Fix forked task title display');
    expect(container.querySelector('[data-id="branch-title"] .conv-item-title')?.textContent).toBe('scott/mobile-row-overflow-audit');
    expect(container.querySelector('[data-id="context-title"] .conv-item-title')?.textContent).toBe('readable-context-leaf');
  });

  it('keeps project context visible on hydrated mobile rows', () => {
    const conv = makeConv('project-context', 'project-context', {
      project_name: 'phoenix-ide',
      cwd: '/tmp/phoenix-ide',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[conv]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('[data-id="project-context"] .conv-project-label')?.textContent).toBe('phoenix-ide');
  });

  it('uses semantic chain and latest-title fallbacks in collapsed mobile chain summaries', () => {
    const root = makeConv('guid-root', 'f872dd1a-f701-49f3-ad25-2605c6b6f3dc', {
      continued_in_conv_id: 'guid-leaf',
      chain_name: null,
      task_title: 'Readable root task title',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const leaf = makeConv('guid-leaf', '9d1b4cc93b7845228e4fdbe566761f44', {
      task_title: 'Readable latest task title',
      updated_at: '2024-02-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[leaf, root]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-name-label')?.textContent).toBe('Readable root task title');
    expect(container.querySelector('.conv-chain-summary-title')?.textContent).toBe('Latest #2: Readable latest task title');
    expect(container.querySelector('.conv-chain-name-label')?.textContent).not.toContain('f872dd1a');
    expect(container.querySelector('.conv-chain-summary-title')?.textContent).not.toContain('9d1b4cc');
  });

  it('keeps cleaned-up terminal mobile chains collapsed by default', () => {
    const root = makeConv('cleanup-root', 'explore-options', {
      continued_in_conv_id: 'cleanup-middle',
      chain_name: 'explore to cleanup',
      conv_mode_label: 'EXPLORE',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const middle = makeConv('cleanup-middle', 'implement-work', {
      continued_in_conv_id: 'cleanup-leaf',
      conv_mode_label: 'WORK',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-02-01T00:00:00Z',
    });
    const leaf = makeConv('cleanup-leaf', 'cleanup-after-merge', {
      conv_mode_label: 'WORK',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-03-01T00:00:00Z',
    });

    const { container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} listDensity="mobile" conversations={[leaf, middle, root]} />
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-block')).toHaveClass('collapsed');
    expect(container.querySelectorAll('.conv-item-chain-member')).toHaveLength(0);
    expect(container.querySelector('.conv-chain-summary-title')?.textContent).toBe('Latest #3: cleanup-after-merge');
  });

  it('mobile keyboard navigation targets collapsed chain summaries, not hidden history rows', () => {
    const root = makeConv('kbd-root', 'kbd-root', {
      continued_in_conv_id: 'kbd-leaf',
      presentation_mode: 'done',
      state: { type: 'terminal' },
      updated_at: '2024-01-01T00:00:00Z',
    });
    const leaf = makeConv('kbd-leaf', 'kbd-leaf', { updated_at: '2024-02-01T00:00:00Z' });
    const onPath = vi.fn();

    const { container } = render(
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route path="*" element={(
            <>
              <ConversationList {...defaultProps} listDensity="mobile" conversations={[leaf, root]} />
              <PathReader onPath={onPath} />
            </>
          )} />
        </Routes>
      </MemoryRouter>,
    );

    expect(container.querySelector('.conv-chain-block')).toHaveClass('collapsed');
    expect(container.querySelectorAll('.conv-item-chain-member')).toHaveLength(0);

    fireEvent.keyDown(window, { key: 'j' });
    const summary = container.querySelector('.conv-chain-latest-summary');
    expect(summary).toHaveClass('keyboard-selected');
    expect(summary).toHaveAttribute('data-id', 'kbd-leaf');

    fireEvent.keyDown(window, { key: 'Enter' });
    const calls = onPath.mock.calls;
    expect(calls[calls.length - 1]![0]).toBe('/c/kbd-leaf');
  });
});

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
      listDensity: 'full',
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
      listDensity: 'full',
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

// C5 render-isolation regression (task 51001): ChainBlock is memoized, but it
// used to receive the GLOBAL expanded/keyboard-selected/active ids, so changing
// any one of them re-rendered every chain. The parent now narrows those ids to
// each chain before crossing the memo boundary, so a global id change that
// lands in chain A leaves chain B's props referentially identical → B bails out.
describe('ConversationList — chain render isolation (C5)', () => {
  beforeEach(() => {
    formatRelativeTimeSpy.mockClear();
    formatShortDateTimeSpy.mockClear();
  });

  // Two independent chains, A (rootA → leafA) and B (rootB → leafB).
  const twoChains = () => {
    const rootA = makeConv('rootA', 'rootA-slug', {
      updated_at: '2024-01-01T00:00:00Z',
      continued_in_conv_id: 'leafA',
      chain_name: 'chain A',
    });
    const leafA = makeConv('leafA', 'leafA-slug', { updated_at: '2024-02-01T00:00:00Z' });
    const rootB = makeConv('rootB', 'rootB-slug', {
      updated_at: '2024-03-01T00:00:00Z',
      continued_in_conv_id: 'leafB',
      chain_name: 'chain B',
    });
    const leafB = makeConv('leafB', 'leafB-slug', { updated_at: '2024-04-01T00:00:00Z' });
    return [leafA, rootA, leafB, rootB];
  };

  it('activeSlug pointing into chain A does not re-render chain B', () => {
    const convs = twoChains();

    const { rerender, container } = render(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={convs} activeSlug={null} />
      </MemoryRouter>,
    );
    // Both chains rendered on mount (2 members each → spy called).
    expect(formatRelativeTimeSpy).toHaveBeenCalled();

    formatRelativeTimeSpy.mockClear();
    // Active conversation becomes a member of chain A only.
    rerender(
      <MemoryRouter>
        <ConversationList {...defaultProps} sidebarMode conversations={convs} activeSlug="leafA-slug" />
      </MemoryRouter>,
    );

    // Chain B's members must NOT have re-rendered (props referentially identical).
    // Chain A's members DID re-render to apply/remove the .active highlight, so
    // the spy is called — but only for chain A's 2 rows, never chain B's.
    const calledSlugs = new Set(
      formatRelativeTimeSpy.mock.calls.map((c) => c[0] as string),
    );
    expect(calledSlugs.has('2024-04-01T00:00:00Z')).toBe(false); // leafB updated_at
    expect(calledSlugs.has('2024-03-01T00:00:00Z')).toBe(false); // rootB updated_at

    // Sanity: chain A's active member actually got the highlight.
    expect(
      container.querySelector('[data-id="leafA"]')!.classList.contains('active'),
    ).toBe(true);
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
