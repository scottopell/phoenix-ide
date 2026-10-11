import { readFileSync } from 'node:fs';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { ConversationMarkdownAnchor } from './conversationMarkdown';

function CurrentLocation() {
  const location = useLocation();
  return (
    <>
      <output data-testid="location">{`${location.pathname}${location.search}${location.hash}`}</output>
      <output data-testid="location-state">{JSON.stringify(location.state)}</output>
    </>
  );
}

function renderAnchor(href: string, onFileClick?: (path: string) => void) {
  render(
    <MemoryRouter initialEntries={['/global/coordinator']}>
      <ConversationMarkdownAnchor href={href} onFileClick={onFileClick}>Citation</ConversationMarkdownAnchor>
      <CurrentLocation />
    </MemoryRouter>,
  );
}

describe('ConversationMarkdownAnchor', () => {
  it.each(['/c/source#message-id', 'https://example.com/source'])('adds inline affordance without replacing caller props for %s', (href) => {
    const onClick = vi.fn((event: React.MouseEvent<HTMLAnchorElement>) => event.preventDefault());
    render(
      <MemoryRouter initialEntries={['/global/coordinator']}>
        <ConversationMarkdownAnchor href={href} className="citation" title="Source" onClick={onClick}>
          Citation
        </ConversationMarkdownAnchor>
        <a href="https://example.com/other" className="button">Unrelated action</a>
        <CurrentLocation />
      </MemoryRouter>,
    );

    const link = screen.getByRole('link', { name: 'Citation' });
    expect(link).toHaveClass('citation', 'conversation-markdown-link');
    expect(link).toHaveAttribute('href', href);
    expect(link).toHaveAttribute('title', 'Source');
    expect(screen.getByRole('link', { name: 'Unrelated action' })).not.toHaveClass('conversation-markdown-link');
    fireEvent.click(link);
    expect(onClick).toHaveBeenCalledOnce();
    expect(screen.getByTestId('location')).toHaveTextContent('/global/coordinator');
  });

  it('preserves native modified-click behavior for in-app links', () => {
    renderAnchor('/c/source#message-id');

    fireEvent.click(screen.getByRole('link', { name: 'Citation' }), { ctrlKey: true });
    expect(screen.getByTestId('location')).toHaveTextContent('/global/coordinator');
  });

  it('navigates app-relative conversation message citations in the current Phoenix context', () => {
    renderAnchor('/c/source-conversation#message-message-id');

    const link = screen.getByRole('link', { name: 'Citation' });
    expect(link).not.toHaveAttribute('target');
    fireEvent.click(link);
    expect(screen.getByTestId('location')).toHaveTextContent('/c/source-conversation#message-message-id');
    expect(screen.getByTestId('location-state')).toHaveTextContent(
      JSON.stringify({ conversationReturnOrigin: { kind: 'coordinator', href: '/global/coordinator' } }),
    );
  });

  it('treats same-origin absolute URLs as in-app destinations', () => {
    renderAnchor(`${window.location.origin}/c/source?view=chat#message-message-id`);

    const link = screen.getByRole('link', { name: 'Citation' });
    expect(link).not.toHaveAttribute('target');
    fireEvent.click(link);
    expect(screen.getByTestId('location')).toHaveTextContent('/c/source?view=chat#message-message-id');
  });

  it('does not attach a Coordinator return origin to unrelated app-local links', () => {
    render(
      <MemoryRouter initialEntries={['/c/current']}>
        <ConversationMarkdownAnchor href="/c/source">Citation</ConversationMarkdownAnchor>
        <CurrentLocation />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('link', { name: 'Citation' }));
    expect(screen.getByTestId('location-state')).toHaveTextContent('null');
  });

  it('opens external destinations in a separate safe browsing context', () => {
    renderAnchor('https://example.com/source');

    expect(screen.getByRole('link', { name: 'Citation' })).toHaveAttribute('target', '_blank');
    expect(screen.getByRole('link', { name: 'Citation' })).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('preserves local file-path viewer behavior', () => {
    const onFileClick = vi.fn();
    renderAnchor('/work/phoenix/src/main.rs', onFileClick);

    expect(screen.queryByRole('link', { name: 'Citation' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Citation' })).not.toHaveClass('conversation-markdown-link');
    fireEvent.click(screen.getByRole('button', { name: 'Citation' }));
    expect(onFileClick).toHaveBeenCalledWith('/work/phoenix/src/main.rs');
  });
});

describe('ConversationMarkdownAnchor CSS affordance', () => {
  const css = readFileSync('src/components/conversationMarkdown.css', 'utf8');
  const inlineSelector = "a.conversation-markdown-link[href]:not([role='button'])";

  it('underlines inline links without hover, including on touch, with theme-aware color', () => {
    const baseRule = css.slice(css.indexOf(`${inlineSelector} {`), css.indexOf('}'));
    expect(baseRule).toContain('color: color-mix(in srgb, var(--accent-blue) 90%, var(--text-primary))');
    expect(baseRule).toContain('text-decoration-line: underline');
    expect(baseRule).toContain('text-decoration-thickness: 1px');
    expect(baseRule).toContain('text-underline-offset: 0.15em');
    expect(css).not.toContain('@media');
  });

  it.each(['dark', 'light'])('has at least 4.5:1 text contrast on %s conversation surfaces', (theme) => {
    const appCss = readFileSync('src/index.css', 'utf8');
    const themeRule = appCss.match(new RegExp(`\\[data-theme="${theme}"\\]\\s*\\{([^}]+)`))?.[1] ?? '';
    const rgb = (token: string) => {
      const hex = themeRule.match(new RegExp(`${token}:\\s*#([a-f0-9]+)`, 'i'))?.[1] ?? '';
      expect(hex.length).toBeGreaterThan(0);
      const expanded = hex.length === 3 ? [...hex].map((digit) => digit + digit).join('') : hex;
      return [0, 2, 4].map((offset) => parseInt(expanded.slice(offset, offset + 2), 16) / 255);
    };
    const luminance = (color: number[]) => color.reduce((sum, channel, index) => {
      const linear = channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
      return sum + linear * ([0.2126, 0.7152, 0.0722][index] ?? 0);
    }, 0);
    const blue = rgb('--accent-blue');
    const text = rgb('--text-primary');
    const linkLuminance = luminance(blue.map((channel, index) => channel * 0.9 + (text[index] ?? 0) * 0.1));
    for (const surface of ['--bg-primary', '--bg-secondary', '--bg-tertiary']) {
      const backgroundLuminance = luminance(rgb(surface));
      const contrast = (Math.max(linkLuminance, backgroundLuminance) + 0.05)
        / (Math.min(linkLuminance, backgroundLuminance) + 0.05);
      expect(contrast).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('keeps all rules scoped to inline links and excludes button semantics', () => {
    const selectors = [...css.matchAll(/([^{}]+)\{[^{}]*\}/g)].flatMap((match) => (match[1] ?? '').trim().split(/,\s*/));
    expect(selectors.length).toBeGreaterThan(0);
    for (const selector of selectors) {
      expect(selector).toMatch(/^a\.conversation-markdown-link\[href\]:not\(\[role='button'\]\)/);
    }
  });

  it('provides a visible keyboard focus outline independent of hover', () => {
    const focusRule = css.slice(css.indexOf(`${inlineSelector}:focus-visible {`));
    expect(focusRule).toContain('outline: 2px solid currentcolor');
    expect(focusRule).toContain('outline-offset: 3px');
  });
});
