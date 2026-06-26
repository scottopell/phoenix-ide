import mermaid from 'mermaid';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
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

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
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
