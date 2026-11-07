import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act, fireEvent, screen } from '@testing-library/react';
import { useState } from 'react';
import { DirectoryPicker } from './DirectoryPicker';
import { api } from '../api';

vi.mock('../api', () => ({
  api: {
    validateCwd: vi.fn(),
    listDirectory: vi.fn().mockResolvedValue({ entries: [] }),
  },
}));

describe('DirectoryPicker validation', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('ignores stale validation responses after cwd changes', async () => {
    let resolveOld: (value: { valid: boolean; is_git: boolean }) => void = () => undefined;
    let resolveNew: (value: { valid: boolean; is_git: boolean }) => void = () => undefined;
    vi.mocked(api.validateCwd)
      .mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve; }))
      .mockImplementationOnce(() => new Promise(resolve => { resolveNew = resolve; }));

    const gitChanges: Array<boolean | null> = [];
    const { rerender } = render(
      <DirectoryPicker
        value="/old"
        onChange={vi.fn()}
        onStatusChange={vi.fn()}
        onGitStatusChange={(isGit) => gitChanges.push(isGit)}
      />,
    );

    await act(async () => {
      vi.advanceTimersByTime(300);
    });

    rerender(
      <DirectoryPicker
        value="/new"
        onChange={vi.fn()}
        onStatusChange={vi.fn()}
        onGitStatusChange={(isGit) => gitChanges.push(isGit)}
      />,
    );

    await act(async () => {
      vi.advanceTimersByTime(300);
    });

    await act(async () => {
      resolveNew({ valid: true, is_git: true });
      await Promise.resolve();
    });
    await act(async () => {
      resolveOld({ valid: true, is_git: false });
      await Promise.resolve();
    });

    expect(gitChanges.at(-1)).toBe(true);
  });

  it('accepts multiple missing directory segments when an ancestor exists', async () => {
    vi.mocked(api.validateCwd).mockImplementation(async (path) => ({
      valid: path === '/existing',
      is_git: false,
    }));
    const statuses: string[] = [];

    render(
      <DirectoryPicker
        value="/existing/missing/nested"
        onChange={vi.fn()}
        onStatusChange={(status) => statuses.push(status)}
        onGitStatusChange={vi.fn()}
      />,
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });

    expect(statuses.at(-1)).toBe('will-create');
    expect(api.validateCwd).toHaveBeenCalledWith('/existing/missing/nested');
  });
});

function RecentPicker({ recentPaths, initialPath = '/selected' }: { recentPaths: string[]; initialPath?: string }) {
  const [path, setPath] = useState(initialPath);
  return <DirectoryPicker value={path} onChange={setPath} recentPaths={recentPaths} />;
}

describe('DirectoryPicker recent locations', () => {
  beforeEach(() => {
    vi.mocked(api.validateCwd).mockResolvedValue({ valid: true, is_git: true });
  });

  it('keeps the selected path editable without showing a picker when there are no suggestions', () => {
    render(<RecentPicker recentPaths={[]} />);
    expect(screen.getByRole('textbox', { name: 'Directory path' })).toHaveValue('/selected');
    expect(screen.queryByRole('button', { name: /Choose recent location/ })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole('textbox', { name: 'Directory path' }), { target: { value: '/outside/recent' } });
    expect(screen.getByRole('textbox', { name: 'Directory path' })).toHaveValue('/outside/recent');
  });

  it('keeps a short suggestion set collapsed until requested', () => {
    render(<RecentPicker recentPaths={['/selected', '/second']} />);
    expect(screen.queryByRole('button', { name: '/second' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Choose recent location (2)' }));
    fireEvent.click(screen.getByRole('button', { name: '/second' }));
    expect(screen.getByRole('textbox', { name: 'Directory path' })).toHaveValue('/second');
  });

  it('discloses every returned location, filters paths, and restores focus after selection', () => {
    const paths = Array.from({ length: 20 }, (_, i) => `/repositories/project-${i + 1}`);
    render(<RecentPicker recentPaths={paths} />);
    const toggle = screen.getByRole('button', { name: 'Choose recent location (20)' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('searchbox', { name: 'Search recent locations' })).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    for (const path of paths) expect(screen.getByRole('button', { name: path })).toBeInTheDocument();
    const search = screen.getByRole('searchbox', { name: 'Search recent locations' });
    expect(search).toHaveFocus();
    fireEvent.change(search, { target: { value: 'project-20' } });
    expect(screen.queryByRole('button', { name: paths[0] })).not.toBeInTheDocument();
    const lastPath = paths.at(-1)!;
    const selection = screen.getByRole('button', { name: lastPath });
    fireEvent.click(selection);
    expect(screen.getByRole('textbox', { name: 'Directory path' })).toHaveValue(lastPath);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveFocus();
    fireEvent.click(toggle);
    expect(screen.getByRole('button', { name: paths[0] })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: lastPath })).toHaveAttribute('aria-current', 'true');
  });

  it('shows a no-match state, and Escape closes search without changing the selected path', () => {
    render(<RecentPicker recentPaths={['/first', '/second']} />);
    const toggle = screen.getByRole('button', { name: 'Choose recent location (2)' });
    fireEvent.click(toggle);
    const search = screen.getByRole('searchbox', { name: 'Search recent locations' });
    fireEvent.change(search, { target: { value: 'missing' } });
    expect(screen.getByText('No matching locations')).toBeInTheDocument();
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveFocus();
    fireEvent.click(toggle);
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search recent locations' }), { target: { value: '' } });
    const result = screen.getByRole('button', { name: '/first' });
    result.focus();
    fireEvent.keyDown(result, { key: 'Escape' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveFocus();
    expect(screen.getByRole('textbox', { name: 'Directory path' })).toHaveValue('/selected');
  });
});
