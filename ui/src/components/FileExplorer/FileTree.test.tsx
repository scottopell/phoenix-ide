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
import { computeAncestors, isUnderRoot } from './computeAncestors';

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

describe('isUnderRoot', () => {
  it('true for root itself, files at root, and deep descendants', () => {
    expect(isUnderRoot('/proj', '/proj')).toBe(true);
    expect(isUnderRoot('/proj', '/proj/README.md')).toBe(true);
    expect(isUnderRoot('/proj', '/proj/ui/src/x.ts')).toBe(true);
  });

  it('handles trailing slash on rootPath', () => {
    expect(isUnderRoot('/proj/', '/proj/x.ts')).toBe(true);
  });

  it('false for sibling-prefix paths and unrelated paths', () => {
    // /proj-other shares the textual prefix /proj but is NOT under it —
    // catches the naive `startsWith(rootPath)` bug.
    expect(isUnderRoot('/proj', '/proj-other/x.ts')).toBe(false);
    expect(isUnderRoot('/proj', '/elsewhere/x.ts')).toBe(false);
  });
});

describe('FileTree — reveal active file', () => {
  let scrollSpy: ReturnType<typeof vi.fn>;
  // Capture happy-dom's pre-existing descriptor so afterEach can restore
  // the prototype to exactly the state other tests expect — leaving a stub
  // installed on Element.prototype creates cross-test coupling.
  let originalScrollDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    installFetchMock();
    // happy-dom does not implement scrollIntoView; install a spy stub.
    originalScrollDescriptor = Object.getOwnPropertyDescriptor(
      Element.prototype,
      'scrollIntoView',
    );
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
    if (originalScrollDescriptor) {
      Object.defineProperty(Element.prototype, 'scrollIntoView', originalScrollDescriptor);
    } else {
      // happy-dom didn't have a descriptor; remove the stub so we don't
      // leave an unowned property on Element.prototype.
      delete (Element.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView;
    }
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

  it('does NOT scroll or expand when activeFile is outside rootPath', async () => {
    // Guards against the dropped-comment regression: out-of-root activeFile
    // used to leave lastRevealedRef = null, so the scroll effect would
    // re-query (and fail) on every childItems update. Now the reveal effect
    // marks it as already-revealed and the scroll effect short-circuits.
    const onFileSelect = vi.fn();
    render(
      <FileTree
        rootPath="/proj"
        onFileSelect={onFileSelect}
        activeFile="/elsewhere/some-file.ts"
        conversationId="conv-test-2"
      />,
    );

    await screen.findByText('README.md');
    // Sanity: nothing under /proj/ui got auto-expanded by an out-of-root file.
    expect(screen.queryByText('FileTree.tsx')).not.toBeInTheDocument();
    // And no scroll was attempted on the active row (because there is none).
    expect(scrollSpy).not.toHaveBeenCalled();
  });
});
