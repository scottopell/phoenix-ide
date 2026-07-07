// Tests for the sidebar conversation list idempotency helper.
// See `conversationDiff.ts` and task 02703.

import { describe, it, expect } from 'vitest';
import type { Conversation } from '../api';
import { conversationListsEqual } from './conversationDiff';

function makeConv(id: string, updated_at: string, overrides: Partial<Conversation> = {}): Conversation {
  return {
    id,
    slug: `slug-${id}`,
    model: 'm',
    cwd: '/',
    created_at: updated_at,
    updated_at,
    message_count: 0,
    state: { type: 'idle' },
    archived: false,
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: `conversation:${id}`,
    ...overrides,
  };
}

describe('conversationListsEqual', () => {
  it('returns true for the same array reference (fast path)', () => {
    const list = [makeConv('a', '2025-01-01T00:00:00Z')];
    expect(conversationListsEqual(list, list)).toBe(true);
  });

  it('returns true when contents match by (id, slug, updated_at)', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z'), makeConv('b', '2025-01-02T00:00:00Z')];
    const b = [makeConv('a', '2025-01-01T00:00:00Z'), makeConv('b', '2025-01-02T00:00:00Z')];
    expect(conversationListsEqual(a, b)).toBe(true);
  });

  it('detects updated_at changes (the canonical “something changed” signal)', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z')];
    const b = [makeConv('a', '2025-01-01T00:00:01Z')];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('detects reordering (server moves a row to top after activity)', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z'), makeConv('b', '2025-01-02T00:00:00Z')];
    const b = [makeConv('b', '2025-01-02T00:00:00Z'), makeConv('a', '2025-01-01T00:00:00Z')];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('detects length changes', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z')];
    const b = [makeConv('a', '2025-01-01T00:00:00Z'), makeConv('b', '2025-01-02T00:00:00Z')];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('detects id swap (rename produces a new slug; updated_at moves with it)', () => {
    // Even at the same updated_at (improbable but possible at clock granularity),
    // a new id must register as a change.
    const a = [makeConv('a', '2025-01-01T00:00:00Z')];
    const b = [makeConv('z', '2025-01-01T00:00:00Z')];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('treats two empty lists as equal', () => {
    expect(conversationListsEqual([], [])).toBe(true);
  });

  it('detects cached PR changes without an updated_at change', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z')];
    const b = [makeConv('a', '2025-01-01T00:00:00Z', {
      cached_pr: {
        number: 12,
        title: 'Cached PR',
        url: 'https://example.test/pr/12',
        display_state: 'open',
        base: 'main',
        head: 'feature',
      },
    })];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('detects slug-only moves without an updated_at change', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z', { slug: 'old' })];
    const b = [makeConv('a', '2025-01-01T00:00:00Z', { slug: 'new' })];
    expect(conversationListsEqual(a, b)).toBe(false);
  });

  it('ignores fields outside (id, slug, updated_at, cached_pr)', () => {
    const a = [makeConv('a', '2025-01-01T00:00:00Z', { message_count: 0 })];
    const b = [makeConv('a', '2025-01-01T00:00:00Z', { message_count: 99 })];
    expect(conversationListsEqual(a, b)).toBe(true);
  });
});
