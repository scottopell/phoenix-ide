// Phoenix Chains v1 — pure-helper tests for sidebar grouping (REQ-CHN-002).
// Algorithm reference: specs/chains/design.md "Sidebar Grouping".

import { describe, it, expect } from 'vitest';
import { computeChainRoots, groupConversationsForSidebar } from './chains';
import type { Conversation } from '../api';

const makeConv = (
  id: string,
  updated_at: string,
  overrides: Partial<Conversation> = {},
): Conversation => ({
  id,
  slug: id,
  model: 'claude-3-5-sonnet',
  cwd: '/tmp',
  created_at: updated_at,
  updated_at,
  message_count: 1,
  ...overrides,
});

describe('computeChainRoots', () => {
  it('maps every member of a 3-conv chain to the same root', () => {
    // a -> b -> c
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'b' });
    const b = makeConv('b', '2024-01-02T00:00:00Z', { continued_in_conv_id: 'c' });
    const c = makeConv('c', '2024-01-03T00:00:00Z');
    const roots = computeChainRoots([a, b, c]);

    expect(roots.get('a')).toBe('a');
    expect(roots.get('b')).toBe('a');
    expect(roots.get('c')).toBe('a');
  });

  it('returns null for a single standalone conversation', () => {
    const a = makeConv('a', '2024-01-01T00:00:00Z');
    const roots = computeChainRoots([a]);
    expect(roots.get('a')).toBeNull();
  });

  it('returns null for a singleton even if continued_in_conv_id points outside the loaded list', () => {
    // Backwards-compat / pagination defense: if the successor isn't loaded,
    // the conversation looks standalone in this view rather than rooted at
    // a phantom successor.
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'ghost' });
    const roots = computeChainRoots([a]);
    expect(roots.get('a')).toBeNull();
  });

  it('handles two parallel chains independently', () => {
    // Chain 1: a -> b
    // Chain 2: x -> y -> z
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'b' });
    const b = makeConv('b', '2024-01-02T00:00:00Z');
    const x = makeConv('x', '2024-02-01T00:00:00Z', { continued_in_conv_id: 'y' });
    const y = makeConv('y', '2024-02-02T00:00:00Z', { continued_in_conv_id: 'z' });
    const z = makeConv('z', '2024-02-03T00:00:00Z');
    const roots = computeChainRoots([a, b, x, y, z]);

    expect(roots.get('a')).toBe('a');
    expect(roots.get('b')).toBe('a');
    expect(roots.get('x')).toBe('x');
    expect(roots.get('y')).toBe('x');
    expect(roots.get('z')).toBe('x');
  });

  it('treats a broken-chain successor as standalone (graceful fallback)', () => {
    // a -> ghost (missing). We do not crash; a is treated as standalone.
    // Its is-chain-root determination requires a loaded successor.
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'ghost' });
    const b = makeConv('b', '2024-01-02T00:00:00Z'); // unrelated standalone
    const roots = computeChainRoots([a, b]);

    expect(roots.get('a')).toBeNull();
    expect(roots.get('b')).toBeNull();
  });

  it('does not loop on a self-cycle (defense-in-depth)', () => {
    // Backend invariants forbid this, but we must not infinite-loop if it
    // ever appears.
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'a' });
    const roots = computeChainRoots([a]);
    expect(roots.has('a')).toBe(true);
  });

  it('returns standalone for a conversation that mixes with archived/standalones', () => {
    // a -> b chain, plus standalone s
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'b' });
    const b = makeConv('b', '2024-01-02T00:00:00Z');
    const s = makeConv('s', '2024-01-05T00:00:00Z');
    const roots = computeChainRoots([a, b, s]);

    expect(roots.get('a')).toBe('a');
    expect(roots.get('b')).toBe('a');
    expect(roots.get('s')).toBeNull();
  });
});

describe('groupConversationsForSidebar', () => {
  it('places a chain block at the recency rank of its most-recent member', () => {
    // Recency-sorted (updated_at DESC):
    //   newer-standalone (2024-04)
    //   chain-leaf       (2024-03)         <- chain block goes here
    //   middle-standalone (2024-02)
    //   chain-root       (2024-01)
    //   older-standalone (2023-12)
    const newerStandalone = makeConv('ns', '2024-04-01T00:00:00Z');
    const chainLeaf = makeConv('cl', '2024-03-01T00:00:00Z');
    const middleStandalone = makeConv('ms', '2024-02-01T00:00:00Z');
    const chainRoot = makeConv('cr', '2024-01-01T00:00:00Z', {
      continued_in_conv_id: 'cl',
      chain_name: 'auth refactor',
    });
    const olderStandalone = makeConv('os', '2023-12-01T00:00:00Z');

    const recencySorted = [newerStandalone, chainLeaf, middleStandalone, chainRoot, olderStandalone];
    const roots = computeChainRoots(recencySorted);
    const grouped = groupConversationsForSidebar(recencySorted, roots);

    // Expect 4 entries: NS, chain, MS, OS. The chain absorbs both chain
    // members; the leaf's recency rank dictates the block's position.
    expect(grouped.length).toBe(4);

    expect(grouped[0]).toMatchObject({ kind: 'single', conversation: { id: 'ns' } });
    expect(grouped[1]?.kind).toBe('chain');
    expect(grouped[2]).toMatchObject({ kind: 'single', conversation: { id: 'ms' } });
    expect(grouped[3]).toMatchObject({ kind: 'single', conversation: { id: 'os' } });

    if (grouped[1]?.kind === 'chain') {
      expect(grouped[1].displayName).toBe('auth refactor');
      expect(grouped[1].rootId).toBe('cr');
      // Members in chain order (root -> leaf) regardless of updated_at.
      expect(grouped[1].members.map(m => m.id)).toEqual(['cr', 'cl']);
      // Latest = max updated_at = leaf.
      expect(grouped[1].latestMemberId).toBe('cl');
    }
  });

  it('Latest skips the root even when the root has the most recent updated_at (matches backend rule)', () => {
    // Real-world case: editing the chain name bumps the root's updated_at
    // via the API's UPDATE on conversations.chain_name. If "Latest" used a
    // pure max(updated_at), the root would incorrectly become Latest after
    // a rename. The backend ChainView (src/api/chains.rs) explicitly picks
    // a non-root member; the sidebar must match.
    const root = makeConv('r', '2024-12-01T00:00:00Z', { continued_in_conv_id: 'l' });
    const leaf = makeConv('l', '2024-06-01T00:00:00Z');

    const recencySorted = [root, leaf];
    const roots = computeChainRoots(recencySorted);
    const grouped = groupConversationsForSidebar(recencySorted, roots);

    expect(grouped.length).toBe(1);
    expect(grouped[0]?.kind).toBe('chain');
    if (grouped[0]?.kind === 'chain') {
      expect(grouped[0].members.map(m => m.id)).toEqual(['r', 'l']);
      expect(grouped[0].latestMemberId).toBe('l'); // leaf, not root
    }
  });

  it('Latest picks the non-root member with max updated_at across multi-offshoot chains', () => {
    // 3-member chain a -> b -> c. Root a has the most recent updated_at
    // (e.g. from a chain_name edit), but among non-root members b is more
    // recent than c. Latest must be b, not a.
    const a = makeConv('a', '2024-12-01T00:00:00Z', { continued_in_conv_id: 'b' });
    const b = makeConv('b', '2024-06-01T00:00:00Z', { continued_in_conv_id: 'c' });
    const c = makeConv('c', '2024-03-01T00:00:00Z');
    const recencySorted = [a, b, c];
    const grouped = groupConversationsForSidebar(recencySorted, computeChainRoots(recencySorted));

    expect(grouped.length).toBe(1);
    if (grouped[0]?.kind === 'chain') {
      expect(grouped[0].members.map(m => m.id)).toEqual(['a', 'b', 'c']);
      expect(grouped[0].latestMemberId).toBe('b');
    }
  });

  it('falls back to root.slug when chain_name is null', () => {
    const root = makeConv('rooty', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'l' });
    const leaf = makeConv('l', '2024-02-01T00:00:00Z');
    const grouped = groupConversationsForSidebar([leaf, root], computeChainRoots([leaf, root]));
    if (grouped[0]?.kind === 'chain') {
      expect(grouped[0].displayName).toBe('rooty');
    }
  });

  it('renders standalone conversations as single items', () => {
    const s1 = makeConv('s1', '2024-02-01T00:00:00Z');
    const s2 = makeConv('s2', '2024-01-01T00:00:00Z');
    const grouped = groupConversationsForSidebar([s1, s2], computeChainRoots([s1, s2]));
    expect(grouped.map(i => i.kind)).toEqual(['single', 'single']);
  });

  it('emits each chain block exactly once even if walked from multiple members', () => {
    // a -> b -> c, listed in arbitrary recency order.
    const a = makeConv('a', '2024-01-01T00:00:00Z', { continued_in_conv_id: 'b' });
    const b = makeConv('b', '2024-03-01T00:00:00Z', { continued_in_conv_id: 'c' });
    const c = makeConv('c', '2024-02-01T00:00:00Z');
    // Recency sort: b (Mar) > c (Feb) > a (Jan)
    const recencySorted = [b, c, a];
    const grouped = groupConversationsForSidebar(recencySorted, computeChainRoots(recencySorted));

    expect(grouped.length).toBe(1);
    expect(grouped[0]?.kind).toBe('chain');
    if (grouped[0]?.kind === 'chain') {
      expect(grouped[0].members.map(m => m.id)).toEqual(['a', 'b', 'c']);
      expect(grouped[0].latestMemberId).toBe('b');
    }
  });
});
