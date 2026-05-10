import { describe, it, expect, beforeEach } from 'vitest';
import { render, act } from '@testing-library/react';
import { useState } from 'react';
import { useRecentFiles, type RecentFile } from './useRecentFiles';

function recent(path: string): RecentFile {
  return { path, name: path.split('/').pop() || path, openedAt: 0 };
}

function Probe({ id }: { id: string | undefined }) {
  const { recentFiles } = useRecentFiles(id);
  return (
    <ul data-testid="recent">
      {recentFiles.map((f) => (
        <li key={f.path}>{f.path}</li>
      ))}
    </ul>
  );
}

describe('useRecentFiles — returning navigation does not flash stale list', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  function Harness({ initial }: { initial: string }) {
    const [id, setId] = useState<string>(initial);
    return (
      <>
        <button data-testid="to-a" onClick={() => setId('conv-a')}>A</button>
        <button data-testid="to-b" onClick={() => setId('conv-b')}>B</button>
        <Probe id={id} />
      </>
    );
  }

  it('renders B-only files immediately after A→B switch (no A flash)', () => {
    localStorage.setItem(
      'phoenix:recent-files:conv-a',
      JSON.stringify([recent('/repo/A.ts')]),
    );
    localStorage.setItem(
      'phoenix:recent-files:conv-b',
      JSON.stringify([recent('/repo/B.ts')]),
    );

    const { getByTestId, queryByText } = render(<Harness initial="conv-a" />);
    expect(queryByText('/repo/A.ts')).not.toBeNull();

    act(() => {
      getByTestId('to-b').click();
    });

    const items = getByTestId('recent').querySelectorAll('li');
    expect(items).toHaveLength(1);
    expect(items[0]!.textContent).toBe('/repo/B.ts');
    expect(queryByText('/repo/A.ts')).toBeNull();
  });

  it('renders A-only files immediately on returning A→B→A (no B flash)', () => {
    localStorage.setItem(
      'phoenix:recent-files:conv-a',
      JSON.stringify([recent('/repo/A.ts')]),
    );
    localStorage.setItem(
      'phoenix:recent-files:conv-b',
      JSON.stringify([recent('/repo/B.ts')]),
    );

    const { getByTestId, queryByText } = render(<Harness initial="conv-a" />);
    act(() => {
      getByTestId('to-b').click();
    });
    act(() => {
      getByTestId('to-a').click();
    });

    const items = getByTestId('recent').querySelectorAll('li');
    expect(items).toHaveLength(1);
    expect(items[0]!.textContent).toBe('/repo/A.ts');
    expect(queryByText('/repo/B.ts')).toBeNull();
  });
});
