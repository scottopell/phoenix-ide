/**
 * FileTree reveal-on-active-file behaviour (task 02700).
 *
 * When `activeFile` is set to a path under `rootPath`, every ancestor
 * directory between root and the file should auto-expand and the row
 * should be scrolled into view. Driven by Cmd+P → FileSource →
 * useFileExplorer().openFile in the real app.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import { FileTree } from './FileTree';
import { computeAncestors } from './computeAncestors';

// /api/files/list response shapes — keyed by absolute path.
const FS: Record<string, Array<{
  name: string;
  path: string;
  is_directory: boolean;
  file_type: string;
  is_text_file: boolean;
  is_gitignored: boolean;
}>> = {
  '/proj': [
    { name: 'ui', path: '/proj/ui', is_directory: true, file_type: 'folder', is_text_file: false, is_gitignored: false },
    { name: 'README.md', path: '/proj/README.md', is_directory: false, file_type: 'markdown', is_text_file: true, is_gitignored: false },
  ],
  '/proj/ui': [
    { name: 'src', path: '/proj/ui/src', is_directory: true, file_type: 'folder', is_text_file: false, is_gitignored: false },
  ],
  '/proj/ui/src': [
    { name: 'components', path: '/proj/ui/src/components', is_directory: true, file_type: 'folder', is_text_file: false, is_gitignored: false },
  ],
  '/proj/ui/src/components': [
    { name: 'FileTree.tsx', path: '/proj/ui/src/components/FileTree.tsx', is_directory: false, file_type: 'code', is_text_file: true, is_gitignored: false },
    { name: 'Other.tsx', path: '/proj/ui/src/components/Other.tsx', is_directory: false, file_type: 'code', is_text_file: true, is_gitignored: false },
  ],
};

function installFetchMock() {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const u = new URL(url, 'http://localhost');
      if (u.pathname === '/api/files/list') {
        const path = u.searchParams.get('path') || '';
        const items = FS[path];
        if (!items) {
          return { ok: false, json: async () => ({ error: `unknown ${path}` }) };
        }
        return { ok: true, json: async () => ({ items }) };
      }
      return { ok: false, json: async () => ({ error: 'unhandled' }) };
    }),
  );
}

describe('computeAncestors', () => {
  it('returns the chain root→leaf, exclusive on both ends', () => {
    expect(
      computeAncestors('/proj', '/proj/ui/src/components/FileTree.tsx'),
    ).toEqual(['/proj/ui', '/proj/ui/src', '/proj/ui/src/components']);
  });

  it('handles trailing slash on rootPath', () => {
    expect(
      computeAncestors('/proj/', '/proj/ui/src/x.ts'),
    ).toEqual(['/proj/ui', '/proj/ui/src']);
  });

  it('returns [] for a file directly at root', () => {
    expect(computeAncestors('/proj', '/proj/README.md')).toEqual([]);
  });

  it('returns [] when activeFile is outside rootPath', () => {
    expect(computeAncestors('/proj', '/elsewhere/x.ts')).toEqual([]);
  });
});

describe('FileTree — reveal active file', () => {
  let scrollSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    installFetchMock();
    // happy-dom does not implement scrollIntoView; install a spy stub.
    scrollSpy = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      writable: true,
      value: scrollSpy,
    });
    // Isolate per test — expansion state is persisted to localStorage
    // keyed by conversationId, and a stale entry from another test
    // would defeat the "initially collapsed" precondition below.
    localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    cleanup();
  });

  it('expands ancestor directories and scrolls the active row into view', async () => {
    const onFileSelect = vi.fn();
    const { rerender } = render(
      <FileTree
        rootPath="/proj"
        onFileSelect={onFileSelect}
        activeFile={null}
        conversationId="conv-test-1"
      />,
    );

    // Wait for the root listing to render — README.md is a leaf at root.
    await screen.findByText('README.md');
    // Initially, the deeply-nested file is NOT in the DOM (its ancestors
    // are all collapsed).
    expect(screen.queryByText('FileTree.tsx')).not.toBeInTheDocument();

    // Now set activeFile — simulates Cmd+P opening the file.
    rerender(
      <FileTree
        rootPath="/proj"
        onFileSelect={onFileSelect}
        activeFile="/proj/ui/src/components/FileTree.tsx"
        conversationId="conv-test-1"
      />,
    );

    // Each ancestor's children get fetched in turn; the leaf file row
    // appears once /proj/ui/src/components is loaded.
    const row = await screen.findByText('FileTree.tsx');
    expect(row).toBeInTheDocument();

    // The row carries the active class.
    const rowEl = row.closest('.ft-item') as HTMLElement | null;
    expect(rowEl).not.toBeNull();
    expect(rowEl!.classList.contains('ft-item--active')).toBe(true);

    // The sibling Other.tsx — in the same directory — is now reachable in
    // the tree. This is the user-visible payoff of the feature.
    expect(screen.getByText('Other.tsx')).toBeInTheDocument();

    // scrollIntoView was called on the active row.
    await waitFor(() => {
      expect(scrollSpy).toHaveBeenCalled();
    });
    // The spy is on Element.prototype, so `this` is the element it was
    // invoked on.
    const calledOn = scrollSpy.mock.contexts[0] as HTMLElement;
    expect(calledOn).toBe(rowEl);
  });
});
