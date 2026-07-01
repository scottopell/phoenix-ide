import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, act } from '@testing-library/react';
import { FileTreeContextMenu } from './FileTreeContextMenu';
import { copyToClipboard } from '../../utils/clipboard';
import {
  FILE_TREE_CONTEXT_MENU_OPEN_EVENT,
  MESSAGE_CONTEXT_MENU_OPEN_EVENT,
  FILE_PATH_CONTEXT_MENU_OPEN_EVENT,
} from '../contextMenuEvents';

vi.mock('../../utils/clipboard', () => ({
  copyToClipboard: vi.fn().mockResolvedValue(true),
}));

function setupTree(rootPath = '/repo/project') {
  const root = document.createElement('div');
  root.className = 'ft-root';
  root.dataset['rootPath'] = rootPath;
  root.innerHTML = `
    <div class="ft-item" data-path="${rootPath}/src/main.rs" data-is-directory="false" data-is-text="true" role="button" tabindex="0">main.rs</div>
    <div class="ft-item" data-path="${rootPath}/src" data-is-directory="true" data-is-text="false" role="button" tabindex="0" aria-expanded="true">src</div>
  `;
  document.body.appendChild(root);
  return root;
}

describe('FileTreeContextMenu', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
    document.querySelectorAll('.ft-root').forEach(el => el.remove());
  });

  it('opens on right-click of a file tree item', () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });

    expect(screen.getByText('Copy relative path')).toBeInTheDocument();
    expect(screen.getByText('Copy absolute path')).toBeInTheDocument();
    expect(screen.getByText('Insert @file reference')).toBeInTheDocument();
    expect(screen.getByText('Insert ./path reference')).toBeInTheDocument();
  });

  it('does not open on shift-right-click', () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100, shiftKey: true });

    expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
  });

  it('does not open outside the file tree', () => {
    const outside = document.createElement('div');
    document.body.appendChild(outside);
    render(<FileTreeContextMenu />);

    fireEvent.contextMenu(outside, { clientX: 100, clientY: 100 });
    expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
    outside.remove();
  });

  it('copies the relative path', () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    fireEvent.click(screen.getByText('Copy relative path'));

    expect(copyToClipboard).toHaveBeenCalledWith('src/main.rs');
  });

  it('copies the absolute path', () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    fireEvent.click(screen.getByText('Copy absolute path'));

    expect(copyToClipboard).toHaveBeenCalledWith('/repo/project/src/main.rs');
  });

  it('dispatches insert-draft event with @file reference', () => {
    const root = setupTree();
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    fireEvent.click(screen.getByText('Insert @file reference'));

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: '@src/main.rs ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('dispatches insert-draft event with ./path reference', () => {
    const root = setupTree();
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    fireEvent.click(screen.getByText('Insert ./path reference'));

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: './src/main.rs ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('closes on Escape', async () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    expect(screen.getByText('Copy relative path')).toBeInTheDocument();

    // The close-on-click-outside/Escape listeners are deferred via setTimeout(0)
    // so the current right-click doesn't immediately close the menu.
    await waitFor(() => {
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
    });
  });

  it('closes on click outside', async () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    expect(screen.getByText('Copy relative path')).toBeInTheDocument();

    await waitFor(() => {
      fireEvent.mouseDown(document.body);
      expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
    });
  });

  it('closes when the message context menu opens', async () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    expect(screen.getByText('Copy relative path')).toBeInTheDocument();

    await act(async () => {
      window.dispatchEvent(new Event(MESSAGE_CONTEXT_MENU_OPEN_EVENT));
    });
    expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
  });

  it('closes when the file path context menu opens', async () => {
    const root = setupTree();
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });
    expect(screen.getByText('Copy relative path')).toBeInTheDocument();

    await act(async () => {
      window.dispatchEvent(new Event(FILE_PATH_CONTEXT_MENU_OPEN_EVENT));
    });
    expect(screen.queryByText('Copy relative path')).not.toBeInTheDocument();
  });

  it('dispatches the file-tree context menu open event', () => {
    const root = setupTree();
    const handler = vi.fn();
    window.addEventListener(FILE_TREE_CONTEXT_MENU_OPEN_EVENT, handler);
    render(<FileTreeContextMenu />);

    const fileItem = root.querySelector('[data-path="/repo/project/src/main.rs"]') as HTMLElement;
    fireEvent.contextMenu(fileItem, { clientX: 100, clientY: 100 });

    expect(handler).toHaveBeenCalledOnce();
    window.removeEventListener(FILE_TREE_CONTEXT_MENU_OPEN_EVENT, handler);
  });
});
