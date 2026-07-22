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
    fireEvent.click(screen.getByRole('button', { name: 'Citation' }));
    expect(onFileClick).toHaveBeenCalledWith('/work/phoenix/src/main.rs');
  });
});
