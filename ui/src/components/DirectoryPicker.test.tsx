import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act } from '@testing-library/react';
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
});
