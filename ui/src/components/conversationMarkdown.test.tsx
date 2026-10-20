import { readFileSync } from 'node:fs';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import { CONVERSATION_MARKDOWN_URL_TRANSFORM, createConversationMarkdownComponents } from './conversationMarkdownImages';
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
  return render(
    <MemoryRouter initialEntries={['/global/coordinator']}>
      <ConversationMarkdownAnchor href={href} onFileClick={onFileClick}>Citation</ConversationMarkdownAnchor>
      <CurrentLocation />
    </MemoryRouter>,
  );
}

describe('ConversationMarkdownAnchor', () => {
  it.each(['@conv:javascript:alert(1)', '@conv:../private', '@conv:58cff20d-81c3-4655-b9d8-61fe615bda28/extra', 'javascript:alert(1)'])('does not promote unsafe or malformed typed href %s', (href) => {
    render(<MemoryRouter><ReactMarkdown components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>{`[unsafe](${href})`}</ReactMarkdown></MemoryRouter>);
    expect(document.querySelector('a[href^="/c/"]')).toBeNull();
  });

  it('routes the actual typed conversation citation through the Markdown sanitizer and router', () => {
    render(<MemoryRouter initialEntries={['/global/coordinator']}>
      <ReactMarkdown components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>
        {'[UI testing conversation](@conv:58cff20d-81c3-4655-b9d8-61fe615bda28)'}
      </ReactMarkdown>
      <CurrentLocation />
    </MemoryRouter>);
    const link = screen.getByRole('link', { name: 'UI testing conversation' });
    expect(link).toHaveAttribute('href', '/c/58cff20d-81c3-4655-b9d8-61fe615bda28');
    expect(link).not.toHaveAttribute('target');
    fireEvent.click(link);
    expect(screen.getByTestId('location')).toHaveTextContent('/c/58cff20d-81c3-4655-b9d8-61fe615bda28');
    expect(screen.getByTestId('location-state')).toHaveTextContent('/global/coordinator');
  });

  it.each(['', '#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2'])('routes the literal transcript citation with exact pinning and return state (%s)', (fragment) => {
    const transcript = 'aceb02b4-0735-491e-bd5c-854e844f1591';
    const origin = '/global/coordinator?viewer=inspect#message-origin';
    const destination = `/c/${transcript}?source_transcript=${transcript}${fragment}`;
    render(<MemoryRouter initialEntries={[origin]}>
      <ReactMarkdown components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>
        {`[Proposal](@transcript:aceb02b4-0735-491e-bd5c-854e844f1591${fragment})`}
      </ReactMarkdown>
      <CurrentLocation />
    </MemoryRouter>);
    const link = screen.getByRole('link', { name: 'Proposal' });
    expect(link).toHaveAttribute('href', destination);
    expect(link).not.toHaveAttribute('target');
    fireEvent.click(link);
    expect(screen.getByTestId('location').textContent).toBe(destination);
    expect(screen.getByTestId('location-state').textContent).toBe(JSON.stringify({
      conversationReturnOrigin: { kind: 'coordinator', href: origin },
    }));
  });

  it.each([
    '@transcript:javascript:alert(1)',
    '@transcript:../private',
    '@transcript:not-a-uuid',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591/../private',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591%2fextra',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591?source_transcript=other',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591#message-invalid',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2/extra',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2#extra',
    'javascript:alert(1)',
    'data:text/html,unsafe',
  ])('keeps unsafe or malformed transcript href sanitized: %s', (href) => {
    render(<MemoryRouter><ReactMarkdown components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>{`[unsafe](${href})`}</ReactMarkdown></MemoryRouter>);
    expect(screen.getByText('unsafe', { selector: 'a' })).toHaveAttribute('href', '');
  });

  it.each([
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591',
    '@transcript:aceb02b4-0735-491e-bd5c-854e844f1591#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2',
  ])('does not promote transcript references used as image sources: %s', (src) => {
    render(<MemoryRouter><ReactMarkdown components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>{`![Proposal](${src})`}</ReactMarkdown></MemoryRouter>);
    expect(screen.getByRole('img', { name: 'Proposal' })).toHaveAttribute('src', '');
  });

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

  it.each([
    '/c/source',
    '/product-conversations/source',
    '/product-conversations/source?viewer=inspect',
    '/product-conversations/source#message-id',
    '/product-conversations/source?source_transcript=old#message-id',
    '/product-conversations/source/',
    '/chains/root',
    '/chains/root?viewer=inspect#message-id',
    '/global',
    '/global?viewer=inspect#message-id',
    '/global/source',
    '/global/source?viewer=inspect#message-id',
  ])('routes supported path %s before local file detection, including same-origin absolute URLs', (path) => {
    for (const href of [path, `${window.location.origin}${path}`]) {
      const onFileClick = vi.fn();
      const view = renderAnchor(href, onFileClick);
      const link = screen.getByRole('link', { name: 'Citation' });
      const destination = path.replace(/\/$/, '');
      expect(link).toHaveAttribute('href', destination);
      expect(link).not.toHaveAttribute('target');
      fireEvent.click(link);
      expect(screen.getByTestId('location').textContent).toBe(destination);
      expect(onFileClick).not.toHaveBeenCalled();
      expect(screen.getByTestId('location-state').textContent).toBe(
        /^\/(?:c|product-conversations)\//.test(path)
          ? JSON.stringify({ conversationReturnOrigin: { kind: 'coordinator', href: '/global/coordinator' } })
          : 'null',
      );
      view.unmount();
    }
  });

  it.each([
    'https://example.com/product-conversations/source?viewer=inspect#message-id',
    '//example.com/product-conversations/source',
    `//${window.location.host}/product-conversations/source`,
    'product-conversations/source',
    './product-conversations/source',
    '../c/source',
    '#message-id',
    '?viewer=inspect',
    'mailto:user@example.com',
    '/c',
    '/chains',
  ])('preserves browser-link behavior for non-app destination %s', (href) => {
    renderAnchor(href);
    const link = screen.getByRole('link', { name: 'Citation' });
    expect(link).toHaveAttribute('href', href);
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it.each([
    '/conversation/source',
    '/product-conversation/source',
    '/product-conversations/source/nested',
    '/c/source/nested',
    '/chains/root/nested',
    '/global/source/nested',
    '/c/src/main.rs',
    './src/main.rs',
    'src/main.rs',
  ])('keeps file candidates and unsupported route shapes in the file viewer: %s', (path) => {
    const onFileClick = vi.fn();
    renderAnchor(path, onFileClick);
    expect(screen.queryByRole('link', { name: 'Citation' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Citation' }));
    expect(onFileClick).toHaveBeenCalledWith(path);
    expect(screen.getByTestId('location').textContent).toBe('/global/coordinator');
  });

  it.each([
    { ctrlKey: true },
    { metaKey: true },
    { shiftKey: true },
    { altKey: true },
    { button: 1 },
  ])('leaves modified or non-primary alias clicks to the browser: %j', (clickOptions) => {
    const onFileClick = vi.fn();
    renderAnchor('/product-conversations/source?viewer=inspect#message-id', onFileClick);
    const link = screen.getByRole('link', { name: 'Citation' });
    const browserDefaultAllowed = fireEvent.click(link, clickOptions);
    expect(browserDefaultAllowed).toBe(true);
    expect(screen.getByTestId('location').textContent).toBe('/global/coordinator');
    expect(onFileClick).not.toHaveBeenCalled();
  });

  it.each(['/c/source', '/product-conversations/source', '/chains/root', '/global', '/global/source'])(
    'emits a directly loadable slashless href for modified clicks and copying %s/', (path) => {
      const destination = `${path}?viewer=inspect&reference=old%3Amember#message-old%3Amsg`;
      for (const href of [`${path}/?viewer=inspect&reference=old%3Amember#message-old%3Amsg`, `${window.location.origin}${path}/?viewer=inspect&reference=old%3Amember#message-old%3Amsg`]) {
        const onFileClick = vi.fn();
        const view = renderAnchor(href, onFileClick);
        const link = screen.getByRole<HTMLAnchorElement>('link', { name: 'Citation' });
        expect(link).toHaveAttribute('href', destination);
        expect(link.href).toBe(`${window.location.origin}${destination}`);
        for (const clickOptions of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
          expect(fireEvent.click(link, clickOptions)).toBe(true);
          expect(screen.getByTestId('location').textContent).toBe('/global/coordinator');
        }
        expect(onFileClick).not.toHaveBeenCalled();
        view.unmount();
      }
    },
  );

  it('supports keyboard-style activation through a native anchor without a file action', () => {
    const onFileClick = vi.fn();
    renderAnchor('/product-conversations/source?viewer=inspect#message-id', onFileClick);
    const link = screen.getByRole('link', { name: 'Citation' });
    link.focus();
    expect(link).toHaveFocus();
    // Native keyboard activation dispatches a primary click with detail 0.
    fireEvent.click(link, { detail: 0 });
    expect(screen.getByTestId('location').textContent).toBe('/product-conversations/source?viewer=inspect#message-id');
    expect(onFileClick).not.toHaveBeenCalled();
  });

  it('preserves the complete Coordinator origin for alias navigation', () => {
    render(
      <MemoryRouter initialEntries={['/global/coordinator?viewer=inspect#message-origin']}>
        <ConversationMarkdownAnchor href="/product-conversations/source">Citation</ConversationMarkdownAnchor>
        <CurrentLocation />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('link', { name: 'Citation' }));
    expect(screen.getByTestId('location-state').textContent).toBe(
      JSON.stringify({ conversationReturnOrigin: { kind: 'coordinator', href: '/global/coordinator?viewer=inspect#message-origin' } }),
    );
  });

  it('keeps code literal and unsafe schemes sanitized through the existing Markdown renderer', () => {
    const onFileClick = vi.fn();
    const { container } = render(
      <MemoryRouter initialEntries={['/global/coordinator']}>
        <ReactMarkdown
          components={createConversationMarkdownComponents({ onFileClick })}
          urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}
        >
          {'[Citation](/product-conversations/source)\n\n`/product-conversations/source`\n\n```text\n[Citation](/product-conversations/source)\n```\n\n[unsafe](javascript:alert%281%29)'}
        </ReactMarkdown>
        <CurrentLocation />
      </MemoryRouter>,
    );
    expect(screen.getAllByRole('link', { name: 'Citation' })).toHaveLength(1);
    expect(container.querySelectorAll('code')).toHaveLength(2);
    expect(container.querySelector('pre code')?.textContent).toContain('[Citation](/product-conversations/source)');
    expect(screen.getByText('unsafe').closest('a')).toHaveAttribute('href', '');
    fireEvent.click(screen.getByRole('link', { name: 'Citation' }));
    expect(screen.getByTestId('location').textContent).toBe('/product-conversations/source');
    expect(onFileClick).not.toHaveBeenCalled();
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

  it.each(['/c/source', '/product-conversations/source'])('does not attach a Coordinator return origin outside Global for %s', (href) => {
    render(
      <MemoryRouter initialEntries={['/c/current']}>
        <ConversationMarkdownAnchor href={href}>Citation</ConversationMarkdownAnchor>
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
