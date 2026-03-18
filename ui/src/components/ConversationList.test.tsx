// Issue regression tests for ConversationList component.
//
// SIDE-02: "All" tab has no project labels
// SIDE-04: Context menu persists across navigation (no click-outside handler)

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
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
    // and .conv-mode-badge (which shows EXPLORE/WORK/STANDALONE).
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
