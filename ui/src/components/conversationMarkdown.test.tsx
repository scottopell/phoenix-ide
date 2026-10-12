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
    fireEvent.click(screen.getByRole('button', { name: 'Citation' }));
    expect(onFileClick).toHaveBeenCalledWith('/work/phoenix/src/main.rs');
  });
});
