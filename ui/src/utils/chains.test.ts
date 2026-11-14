// Phoenix Chains v1 — pure-helper tests for sidebar grouping (REQ-CHN-002).
// Algorithm reference: specs/chains/design.md "Sidebar Grouping".

import { describe, it, expect } from 'vitest';
import { computeChainRoots } from './chains';
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
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: `conversation:${id}`,
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
