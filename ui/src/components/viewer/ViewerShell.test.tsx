import { render, screen } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { ViewerShell } from './ViewerShell';

function renderShell(mode: 'inline' | 'overlay' | 'takeover') {
  const host = document.createElement('div');
  host.dataset['testid'] = 'layout-host';
  document.body.append(host);

  const result = render(
    <ViewerShell
      mode={mode}
      ariaLabel={`${mode} viewer`}
      title="A deliberately long Markdown filename that must yield to viewer actions.md"
      titleTooltip="/tmp/A deliberately long Markdown filename that must yield to viewer actions.md"
      headerExtras={<button type="button">Header action</button>}
      noteCount={0}
      onToggleNotes={vi.fn()}
      onSend={vi.fn()}
      onClose={vi.fn()}
    >
      <p>Viewer body</p>
    </ViewerShell>,
    { container: host },
  );

  return { ...result, host };
}

describe('ViewerShell mounting boundary', () => {
  it('portals takeovers to the document body outside their layout host', () => {
    const { host } = renderShell('takeover');

    const viewer = screen.getByRole('dialog', { name: 'takeover viewer' });
    expect(viewer.parentElement).toHaveClass('viewer-shell-portal-host');
    expect(viewer.parentElement?.parentElement).toBe(document.body);
    expect(host).not.toContainElement(viewer);
    expect(screen.getByRole('button', { name: 'Header action' })).toBeInTheDocument();
    expect(viewer.querySelector('.viewer-shell-title')).toHaveAttribute(
      'title',
      '/tmp/A deliberately long Markdown filename that must yield to viewer actions.md',
    );
  });

  it.each(['inline', 'takeover'] as const)('connects %s children before their layout effects run', (mode) => {
    const connectivity = vi.fn();
    function LayoutProbe() {
      useLayoutEffect(() => connectivity(document.querySelector('[data-layout-probe]')?.isConnected), []);
      return <div data-layout-probe />;
    }

    render(
      <ViewerShell
        mode={mode}
        ariaLabel="measured viewer"
        title="Document"
        noteCount={0}
        onToggleNotes={vi.fn()}
        onSend={vi.fn()}
        onClose={vi.fn()}
      >
        <LayoutProbe />
      </ViewerShell>,
    );

    expect(connectivity).toHaveBeenCalledWith(true);
  });

  it('preserves the scrolled, focused content tree in both presentation directions', () => {
    const host = document.createElement('div');
    document.body.append(host);
    const shell = (mode: 'inline' | 'takeover') => (
      <ViewerShell
        mode={mode}
        ariaLabel="scrolling viewer"
        title="Document"
        noteCount={0}
        onToggleNotes={vi.fn()}
        onSend={vi.fn()}
        onClose={vi.fn()}
      >
        <div className="viewer-content"><input aria-label="Review note" /></div>
      </ViewerShell>
    );
    const view = render(shell('inline'), { container: host });
    const content = host.querySelector('.viewer-content') as HTMLDivElement;
    const input = screen.getByRole('textbox', { name: 'Review note' });
    content.scrollTop = 420;
    input.focus();

    view.rerender(shell('takeover'));

    expect(document.body.querySelector('.viewer-shell--takeover .viewer-content')).toBe(content);
    expect(content.scrollTop).toBe(420);
    expect(input).toHaveFocus();

    content.scrollTop = 760;
    view.rerender(shell('inline'));

    expect(host.querySelector('.viewer-content')).toBe(content);
    expect(content.scrollTop).toBe(760);
    expect(input).toHaveFocus();
  });

  it.each(['inline', 'overlay'] as const)('keeps %s viewers inside their layout host', (mode) => {
    const { host } = renderShell(mode);

    const role = mode === 'inline' ? 'region' : 'dialog';
    const viewer = screen.getByRole(role, { name: `${mode} viewer` });
    expect(host).toContainElement(viewer);
  });
});
