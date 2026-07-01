import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FileTree, FILE_TREE_DRAG_TYPE } from './FileTree';

function stubFilesSimple(path: string, files: { name: string; is_directory: boolean; viewer?: string }[]) {
  const items = files.map(f => ({
    name: f.name,
    path: path.endsWith('/') ? `${path}${f.name}` : `${path}/${f.name}`,
    is_directory: f.is_directory,
    size: 1024,
    modified_time: 1000,
    viewer: { kind: f.viewer ?? 'text' },
    is_gitignored: false,
  }));
  return vi.fn().mockResolvedValue({ ok: true, json: () => Promise.resolve({ items }) });
}

function renderTree(rootPath = '/repo', overrides: { onFileSelect?: (path: string, root: string) => void } = {}) {
  const onFileSelect = overrides.onFileSelect ?? vi.fn();
  return {
    onFileSelect,
    ...render(
      <MemoryRouter>
        <FileTree
          rootPath={rootPath}
          onFileSelect={onFileSelect}
          conversationId="conv-dnd"
        />
      </MemoryRouter>,
    ),
  };
}

describe('FileTree drag-and-drop', () => {
  let visibilitySpy: ReturnType<typeof vi.spyOn>;
  let originalSetTimeout: typeof setTimeout;
  beforeEach(() => {
    visibilitySpy = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    originalSetTimeout = globalThis.setTimeout;
    globalThis.setTimeout = ((fn: () => void, ms?: number) => {
      if (ms !== undefined && ms >= 5000) return 0 as unknown as ReturnType<typeof setTimeout>;
      return originalSetTimeout(fn, ms);
    }) as typeof setTimeout;
    vi.stubGlobal('fetch', stubFilesSimple('/repo', [
      { name: 'src', is_directory: true },
      { name: 'main.rs', is_directory: false },
    ]));
  });

  afterEach(() => {
    globalThis.setTimeout = originalSetTimeout;
    visibilitySpy.mockRestore();
    cleanup();
    vi.unstubAllGlobals();
  });

  it('sets draggable on file items', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('main.rs')).toBeInTheDocument());
    const fileItem = screen.getByText('main.rs').closest('.ft-item') as HTMLElement;
    expect(fileItem.getAttribute('draggable')).toBe('true');
  });

  it('sets the custom drag type on dragStart', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('main.rs')).toBeInTheDocument());
    const fileItem = screen.getByText('main.rs').closest('.ft-item') as HTMLElement;

    const setData = vi.fn();
    const dataTransfer = { setData, effectAllowed: '' };
    fireEvent.dragStart(fileItem, { dataTransfer });

    expect(setData).toHaveBeenCalledWith(
      FILE_TREE_DRAG_TYPE,
      expect.stringContaining('"relativePath":"main.rs"'),
    );
  });

  it('includes isDirectory in the drag payload', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    const dirItem = screen.getByText('src').closest('.ft-item') as HTMLElement;

    const setData = vi.fn();
    const dataTransfer = { setData, effectAllowed: '' };
    fireEvent.dragStart(dirItem, { dataTransfer });

    expect(setData).toHaveBeenCalledWith(
      FILE_TREE_DRAG_TYPE,
      expect.stringContaining('"isDirectory":true'),
    );
  });

  it('prevents drag on disabled (opaque) files', async () => {
    vi.stubGlobal('fetch', stubFilesSimple('/repo', [
      { name: 'binary.bin', is_directory: false, viewer: 'opaque' },
    ]));
    renderTree();
    await waitFor(() => expect(screen.getByText('binary.bin')).toBeInTheDocument());
    const fileItem = screen.getByText('binary.bin').closest('.ft-item') as HTMLElement;

    expect(fileItem.getAttribute('draggable')).toBe('false');
  });
});

describe('FileTree keyboard navigation', () => {
  let visibilitySpy: ReturnType<typeof vi.spyOn>;
  let originalSetTimeout: typeof setTimeout;
  beforeEach(() => {
    visibilitySpy = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    originalSetTimeout = globalThis.setTimeout;
    globalThis.setTimeout = ((fn: () => void, ms?: number) => {
      if (ms !== undefined && ms >= 5000) return 0 as unknown as ReturnType<typeof setTimeout>;
      return originalSetTimeout(fn, ms);
    }) as typeof setTimeout;
    vi.stubGlobal('fetch', stubFilesSimple('/repo', [
      { name: 'src', is_directory: true },
      { name: 'main.rs', is_directory: false },
      { name: 'test.rs', is_directory: false },
    ]));
  });

  afterEach(() => {
    globalThis.setTimeout = originalSetTimeout;
    visibilitySpy.mockRestore();
    cleanup();
    vi.unstubAllGlobals();
  });

  it('moves focus with ArrowDown', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());

    const items = document.querySelectorAll('.ft-item');
    expect(items.length).toBe(3);

    (items[0] as HTMLElement).focus();
    expect(document.activeElement).toBe(items[0]);

    fireEvent.keyDown(items[0], { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);

    fireEvent.keyDown(items[1], { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[2]);
  });

  it('moves focus with ArrowUp', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('test.rs')).toBeInTheDocument());

    const items = document.querySelectorAll('.ft-item');
    (items[2] as HTMLElement).focus();

    fireEvent.keyDown(items[2], { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[1]);

    fireEvent.keyDown(items[1], { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[0]);
  });

  it('blurs on Escape', async () => {
    renderTree();
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());

    const items = document.querySelectorAll('.ft-item');
    (items[0] as HTMLElement).focus();
    expect(document.activeElement).toBe(items[0]);

    fireEvent.keyDown(items[0], { key: 'Escape' });
    expect(document.activeElement).not.toBe(items[0]);
  });
});
