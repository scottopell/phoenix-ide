import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { createMatchId, createSurfaceKey, type FindSessionCommand } from './findSession';
import { useFindSession } from './useFindSession';

type Target = { block: string };
type Origin = { token: string };

const alpha = { id: createMatchId('alpha:0:5'), target: { block: 'alpha' } };
const beta = { id: createMatchId('beta:0:4'), target: { block: 'beta' } };

function openAction(origin = 'button') {
  return {
    type: 'open' as const,
    surface: {
      key: createSurfaceKey('task:1'),
      query: 'a',
      matches: [alpha, beta],
      focusOrigin: { token: origin },
    },
  };
}

describe('useFindSession', () => {
  it('delivers each command batch exactly once', () => {
    const onCommands = vi.fn<(commands: readonly FindSessionCommand<Target, Origin>[]) => void>();
    const { result, rerender } = renderHook(() => useFindSession<Target, Origin>({ onCommands }));

    act(() => result.current.send(openAction()));
    expect(result.current.state.status).toBe('open');
    expect(onCommands).toHaveBeenCalledTimes(1);
    expect(onCommands.mock.calls[0]?.[0]).toEqual([
      { kind: 'focus-query', focusVersion: 1 },
      { kind: 'reveal-match', matchId: alpha.id, target: alpha.target },
    ]);

    rerender();
    expect(onCommands).toHaveBeenCalledTimes(1);

    act(() => result.current.send({ type: 'next' }));
    expect(onCommands).toHaveBeenCalledTimes(2);
    expect(onCommands.mock.calls[1]?.[0]).toEqual([
      { kind: 'reveal-match', matchId: beta.id, target: beta.target },
    ]);
  });

  it('uses the latest command handler without replaying commands', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { result, rerender } = renderHook(
      ({ onCommands }) => useFindSession<Target, Origin>({ onCommands }),
      { initialProps: { onCommands: first } },
    );

    act(() => result.current.send(openAction()));
    expect(first).toHaveBeenCalledTimes(1);

    rerender({ onCommands: second });
    expect(second).not.toHaveBeenCalled();

    act(() => result.current.send({ type: 'close' }));
    expect(second).toHaveBeenCalledWith([
      { kind: 'clear-decorations' },
      { kind: 'restore-focus', focusOrigin: { token: 'button' } },
    ]);
  });

  it('does not emit commands when surviving results keep the active match', () => {
    const onCommands = vi.fn();
    const { result } = renderHook(() => useFindSession<Target, Origin>({ onCommands }));
    act(() => result.current.send(openAction()));
    onCommands.mockClear();

    act(() => result.current.send({ type: 'replace-results', matches: [beta, alpha] }));
    expect(result.current.state.status).toBe('open');
    if (result.current.state.status !== 'open') throw new Error('expected open session');
    expect(result.current.state.activeMatchId).toBe(alpha.id);
    expect(onCommands).not.toHaveBeenCalled();
  });
});
