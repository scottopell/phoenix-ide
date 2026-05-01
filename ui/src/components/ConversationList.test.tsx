// Issue regression tests for ConversationList component.
//
// SIDE-02: "All" tab has no project labels
// SIDE-04: Context menu persists across navigation (no click-outside handler)
// CHN-Sidebar: chain grouping render (REQ-CHN-002, task 02690 Phase 5)

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { ConversationList } from './ConversationList';
import type { Conversation } from '../api';

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
  ...overrides,
});

const defaultProps = {
  archivedConversations: [] as Conversation[],
  showArchived: false,
  onToggleArchived: vi.fn(),
  onNewConversation: vi.fn(),
  onArchive: vi.fn(),
  onUnarchive: vi.fn(),
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
