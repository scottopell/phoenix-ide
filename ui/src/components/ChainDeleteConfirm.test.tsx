// ChainDeleteConfirm — scope-explicit chain delete confirmation.
//
// Coverage: rendered body lists every member by 1-based chain index,
// surfaces the worktree count when any member owns one, omits the worktree
// bullet otherwise, and produces the expected pluralization.

import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { ChainDeleteConfirm } from './ChainDeleteConfirm';
import type { ChainView, ChainMemberSummary } from '../api';

const makeMember = (
  conv_id: string,
  has_worktree: boolean,
  overrides: Partial<ChainMemberSummary> = {},
): ChainMemberSummary => ({
  conv_id,
  slug: `slug-${conv_id}`,
  title: `Title ${conv_id}`,
  message_count: 4,
  updated_at: '2026-04-29T12:00:00Z',
  position: 'continuation',
  has_worktree,
  ...overrides,
});

const makeChain = (overrides: Partial<ChainView> = {}): ChainView => ({
  root_conv_id: 'r',
  chain_name: 'auth refactor',
  display_name: 'auth refactor',
  archived: false,
  members: [
    makeMember('m1', false, { position: 'root' }),
    makeMember('m2', true),
    makeMember('m3', true, { position: 'latest' }),
  ],
  qa_history: [],
  current_member_count: 3,
  current_total_messages: 12,
  ...overrides,
});

describe('ChainDeleteConfirm', () => {
  it('renders the chain name in the title', () => {
    const { getByRole } = render(
      <ChainDeleteConfirm
        visible
        chain={makeChain()}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(getByRole('heading').textContent).toContain('auth refactor');
  });

  it('lists every member by 1-based chain index and surfaces the worktree count', () => {
    const { container } = render(
      <ChainDeleteConfirm
        visible
        chain={makeChain()}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    const items = container.querySelectorAll('.chain-delete-bullets li');
    expect(items.length).toBe(3);
    expect(items[0]!.textContent).toContain('3 conversations');
    expect(items[0]!.textContent).toContain('#1, #2, #3');
    expect(items[1]!.textContent).toContain('2 git worktrees');
    expect(items[2]!.textContent).toContain('All messages and history');
  });

  it('omits the worktree bullet when no member owns a worktree', () => {
    const chain = makeChain({
      members: [
        makeMember('m1', false, { position: 'root' }),
        makeMember('m2', false, { position: 'latest' }),
      ],
      current_member_count: 2,
    });
    const { container } = render(
      <ChainDeleteConfirm
        visible
        chain={chain}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    const items = container.querySelectorAll('.chain-delete-bullets li');
    expect(items.length).toBe(2);
    expect(items[0]!.textContent).toContain('2 conversations');
    expect(items[0]!.textContent).toContain('#1, #2');
    // No worktree bullet between the conversation count and the
    // history bullet.
    expect(items[1]!.textContent).toContain('All messages and history');
  });

  it('singularizes the worktree label when exactly one member has a worktree', () => {
    const chain = makeChain({
      members: [
        makeMember('m1', false, { position: 'root' }),
        makeMember('m2', true, { position: 'latest' }),
      ],
      current_member_count: 2,
    });
    const { container } = render(
      <ChainDeleteConfirm
        visible
        chain={chain}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    const items = container.querySelectorAll('.chain-delete-bullets li');
    expect(items[1]!.textContent).toContain('1 git worktree');
    expect(items[1]!.textContent).not.toContain('worktrees');
  });

  it('renders nothing when not visible', () => {
    const { container } = render(
      <ChainDeleteConfirm
        visible={false}
        chain={makeChain()}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(container.querySelector('.chain-delete-confirm')).toBeNull();
  });

  it('renders nothing when chain is null', () => {
    const { container } = render(
      <ChainDeleteConfirm
        visible
        chain={null}
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(container.querySelector('.chain-delete-confirm')).toBeNull();
  });
});
