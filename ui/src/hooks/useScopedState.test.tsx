import { describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useScopedState } from './useScopedState';

describe('useScopedState', () => {
  it('resets synchronously when the scope key changes', () => {
    const { result, rerender } = renderHook(
      ({ scope }) => useScopedState<string | null>(scope, null),
      { initialProps: { scope: 'conv-a' } },
    );

    act(() => result.current[1]('conv-a payload'));
    expect(result.current[0]).toBe('conv-a payload');

    rerender({ scope: 'conv-b' });
    expect(result.current[0]).toBeNull();
  });

  it('preserves state across re-renders in the same scope', () => {
    const { result, rerender } = renderHook(
      ({ scope }) => useScopedState(scope, 0),
      { initialProps: { scope: 'conv-a' } },
    );

    act(() => result.current[1](3));
    rerender({ scope: 'conv-a' });

    expect(result.current[0]).toBe(3);
  });
});
