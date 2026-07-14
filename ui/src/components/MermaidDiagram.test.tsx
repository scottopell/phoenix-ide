import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MermaidDiagram } from './MermaidDiagram';

// Real mermaid.render injects a temporary measuring node into <body> with id
// `d${id}`, removes it on success, but on a syntax error throws before cleanup —
// orphaning an error-diagram SVG that inflates page height. This mock reproduces
// that injection so the test exercises the orphan-cleanup path, not a stub.
vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn((id: string, code: string) => {
      const node = document.createElement('div');
      node.id = `d${id}`;
      node.innerHTML = '<svg aria-roledescription="error"></svg>';
      document.body.appendChild(node);
      if (code.includes('boom')) {
        return Promise.reject(new Error('Syntax error in text'));
      }
      node.remove();
      return Promise.resolve({
        svg: `<svg role="img"><text>${code}</text></svg>`,
        bindFunctions: vi.fn(),
      });
    }),
  },
}));

const orphans = () => document.querySelectorAll('[id^="dphoenix-mermaid-"]');
const createObjectURL = vi.fn<(blob: Blob) => string>(() => 'blob:phoenix-mermaid');
const revokeObjectURL = vi.fn<(url: string) => void>();

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('URL', class extends URL {
    static override createObjectURL(blob: Blob) {
      return createObjectURL(blob);
    }

    static override revokeObjectURL(url: string) {
      revokeObjectURL(url);
    }
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('MermaidDiagram fullscreen viewing', () => {
  it('exposes a successful render as a standalone SVG browser document', async () => {
    render(<MermaidDiagram code="flowchart TD\n  A --> B" />);

    expect(screen.queryByRole('link', { name: 'Open Mermaid diagram fullscreen' })).not.toBeInTheDocument();

    const link = await screen.findByRole('link', { name: 'Open Mermaid diagram fullscreen' });
    expect(link).toHaveAttribute('href', 'blob:phoenix-mermaid');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(createObjectURL).toHaveBeenCalledOnce();

    const blob = createObjectURL.mock.calls[0]?.[0];
    expect(blob).toBeInstanceOf(Blob);
    expect(blob?.type).toBe('image/svg+xml');
    expect(await blob?.text()).toContain('flowchart TD');
  });

  it('hides the fullscreen action in source mode and after a render error', async () => {
    const { rerender } = render(<MermaidDiagram code="flowchart TD\n  A --> B" />);
    await screen.findByRole('link', { name: 'Open Mermaid diagram fullscreen' });

    fireEvent.click(screen.getByRole('button', { name: 'Source' }));
    expect(screen.queryByRole('link', { name: 'Open Mermaid diagram fullscreen' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Diagram' }));
    expect(screen.getByRole('link', { name: 'Open Mermaid diagram fullscreen' })).toBeInTheDocument();

    rerender(<MermaidDiagram code="boom not a diagram" />);
    await screen.findByRole('alert');
    expect(screen.queryByRole('link', { name: 'Open Mermaid diagram fullscreen' })).not.toBeInTheDocument();
  });

  it('revokes standalone resources when replacing a render and unmounting', async () => {
    createObjectURL
      .mockReturnValueOnce('blob:first-mermaid')
      .mockReturnValueOnce('blob:second-mermaid');
    const { rerender, unmount } = render(<MermaidDiagram code="flowchart TD\n  A --> B" />);
    expect(await screen.findByRole('link', { name: 'Open Mermaid diagram fullscreen' }))
      .toHaveAttribute('href', 'blob:first-mermaid');

    rerender(<MermaidDiagram code="flowchart LR\n  C --> D" />);
    expect(await screen.findByRole('link', { name: 'Open Mermaid diagram fullscreen' }))
      .toHaveAttribute('href', 'blob:second-mermaid');
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:first-mermaid');

    unmount();
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:second-mermaid');
  });
});

describe('MermaidDiagram orphan node cleanup', () => {
  it('removes the orphaned <body> node once the failure is rendered', async () => {
    render(<MermaidDiagram code="boom not a diagram" />);

    // Wait until the failure has actually been processed (error UI shown). Only
    // then is the assertion meaningful — checking earlier would pass trivially
    // because the async mermaid import has not yet injected the orphan.
    await screen.findByRole('alert');

    expect(orphans()).toHaveLength(0);
  });

  it('does not leak orphan nodes across repeated failures', async () => {
    const { rerender } = render(<MermaidDiagram code="boom one" />);
    await screen.findByRole('alert');
    expect(orphans()).toHaveLength(0);

    rerender(<MermaidDiagram code="boom two" />);
    await screen.findByRole('alert');
    expect(orphans()).toHaveLength(0);
  });
});
