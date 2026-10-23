import { describe, expect, it, vi } from 'vitest';
import { delegateTouchRefresh } from './refreshRoute';
import { assertTouchStreamOwner } from './streamOwner';
const base = 'http://127.0.0.1:4179/fixture';
const live = 'touch-reading-member';
const path = '/api/product-conversations/events';
function fixture() {
  const calls: unknown[][] = [];
  class Base {
    close = vi.fn();
    constructor(...args: unknown[]) { calls.push(args); }
  }
  return { Base: Base as unknown as typeof EventSource, calls };
}
describe('fixture refresh delegation', () => {
  it.each([path, `http://127.0.0.1:4179${path}`])('preserves constructor arguments for %s', input => {
    const { Base, calls } = fixture();
    const options = { withCredentials: false };
    const source = delegateTouchRefresh(Base, [input, options], base);
    expect(source).toBeInstanceOf(Base);
    expect(calls).toEqual([[input, options]]);
    expect(calls[0][1]).toBe(options);
    source!.close();
    expect(source!.close).toHaveBeenCalledTimes(1);
  });
  it.each([
    `http://foreign.invalid${path}`, `${path}/other`, '/api/other',
    `${path}?reference=${live}`, `${path}?reference=${live}&reference=${live}`,
    `${path}?`, `${path}#`, `${path}#fragment`,
    `http://name:secret@127.0.0.1:4179${path}`,
    '/api/conversation-reads/stream?reference=foreign',
  ])('does not delegate invalid/non-refresh input %s', input => {
    const { Base, calls } = fixture();
    expect(delegateTouchRefresh(Base, [input], base)).toBeNull();
    expect(calls).toEqual([]);
    expect(() => assertTouchStreamOwner(input, base, live)).toThrow();
  });
  it('leaves valid conversation input to the unchanged owner guard', () => {
    const { Base, calls } = fixture();
    const input = `/api/conversation-reads/stream?reference=${live}`;
    expect(delegateTouchRefresh(Base, [input], base)).toBeNull();
    expect(calls).toEqual([]);
    expect(() => assertTouchStreamOwner(input, base, live)).not.toThrow();
  });
});
