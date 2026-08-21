import { memo, useState } from 'react';
import { act, render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ThemeProvider } from './ThemeProvider';
import { useTheme } from '../hooks/useTheme';

const ThemeConsumer = memo(function ThemeConsumer({ onRender }: { onRender: () => void }) {
  onRender();
  useTheme();
  return null;
});

function Parent({ onRender }: { onRender: () => void }) {
  const [, setTick] = useState(0);
  return (
    <ThemeProvider>
      <button type="button" onClick={() => setTick((tick) => tick + 1)}>rerender</button>
      <ThemeConsumer onRender={onRender} />
    </ThemeProvider>
  );
}

describe('ThemeProvider render isolation', () => {
  it('synchronizes the browser theme color with the selected theme', () => {
    const meta = document.createElement('meta');
    meta.name = 'theme-color';
    document.head.append(meta);
    localStorage.setItem('phoenix-theme', 'dark');

    function Toggle() {
      const { toggleTheme } = useTheme();
      return <button onClick={toggleTheme}>toggle</button>;
    }

    const { getByRole, unmount } = render(<ThemeProvider><Toggle /></ThemeProvider>);
    expect(meta.content).toBe('#0f1115');
    act(() => getByRole('button', { name: 'toggle' }).click());
    expect(meta.content).toBe('#f8fafc');

    unmount();
    meta.remove();
    localStorage.removeItem('phoenix-theme');
  });

  it('does not broadcast context when its parent re-renders without a theme change', () => {
    const onRender = vi.fn();
    const { getByRole } = render(<Parent onRender={onRender} />);

    expect(onRender).toHaveBeenCalledTimes(1);

    act(() => {
      getByRole('button', { name: 'rerender' }).click();
    });

    expect(onRender).toHaveBeenCalledTimes(1);
  });
});
