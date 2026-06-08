import { memo, useEffect } from 'react';
import { act, render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { FocusScopeProvider, useFocusScope, useRegisterFocusScope } from './useFocusScope';

const CommandOnlyConsumer = memo(function CommandOnlyConsumer({ onRender }: { onRender: () => void }) {
  onRender();
  useRegisterFocusScope('registered-command-consumer');
  return null;
});

function ScopeToggler({ onReady }: { onReady: (commands: ReturnType<typeof useFocusScope>) => void }) {
  const scope = useFocusScope();
  useEffect(() => { onReady(scope); }, [scope, onReady]);
  return null;
}

describe('FocusScopeProvider render isolation', () => {
  it('does not re-render command-only registration consumers when active scope changes', () => {
    const onRender = vi.fn();
    let latestScope: ReturnType<typeof useFocusScope> | null = null;

    render(
      <FocusScopeProvider>
        <CommandOnlyConsumer onRender={onRender} />
        <ScopeToggler onReady={(scope) => { latestScope = scope; }} />
      </FocusScopeProvider>,
    );

    expect(onRender).toHaveBeenCalledTimes(1);

    act(() => {
      latestScope!.pushScope('palette');
    });

    expect(latestScope!.activeScope).toBe('palette');
    expect(onRender).toHaveBeenCalledTimes(1);

    act(() => {
      latestScope!.popScope('palette');
    });

    expect(latestScope!.activeScope).toBe('registered-command-consumer');
    expect(onRender).toHaveBeenCalledTimes(1);
  });
});
