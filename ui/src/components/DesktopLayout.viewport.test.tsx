import { readFileSync } from 'node:fs';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { isViewportOwnedRoute, useDocumentViewportOwnership } from './viewportRoutes';

const appCss = readFileSync(`${process.cwd()}/src/index.css`, 'utf8');

function ruleFor(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = appCss.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`));
  expect(match, `missing CSS rule for ${selector}`).not.toBeNull();
  return match?.[1] ?? '';
}

describe('app viewport ownership', () => {
  it.each(['/new', '/new/', '/c/example', '/c/example/', '/c/with%20space'])(
    'contains the document for the chat shell route %s',
    (pathname) => expect(isViewportOwnedRoute(pathname)).toBe(true),
  );

  it.each(['/', '/about', '/settings/llm-language', '/usage', '/terminal', '/s/token', '/c/', '/c/a/b'])(
    'keeps intentional document-scrolling route %s outside containment',
    (pathname) => expect(isViewportOwnedRoute(pathname)).toBe(false),
  );

  it('applies and releases document containment as route ownership changes', () => {
    function Harness({ ownsViewport }: { ownsViewport: boolean }) {
      useDocumentViewportOwnership(ownsViewport);
      return null;
    }

    const { rerender, unmount } = render(<Harness ownsViewport />);
    expect(document.documentElement).toHaveClass('app-viewport-active');
    expect(document.body).toHaveClass('app-viewport-active');

    rerender(<Harness ownsViewport={false} />);
    expect(document.documentElement).not.toHaveClass('app-viewport-active');
    expect(document.body).not.toHaveClass('app-viewport-active');

    rerender(<Harness ownsViewport />);
    expect(document.documentElement).toHaveClass('app-viewport-active');
    unmount();
    expect(document.documentElement).not.toHaveClass('app-viewport-active');
    expect(document.body).not.toHaveClass('app-viewport-active');
  });

  it('gives chat shells one dynamic viewport owner and contains the document', () => {
    const viewportRule = ruleFor('.app-viewport');
    expect(viewportRule).toMatch(/height:\s*100dvh;/);
    expect(viewportRule).toMatch(/min-height:\s*0;/);
    expect(viewportRule).toMatch(/overflow:\s*hidden;/);

    expect(appCss).toMatch(
      /html\.app-viewport-active,\s*body\.app-viewport-active,\s*body\.app-viewport-active #root\s*\{[^}]*overflow:\s*hidden;[^}]*overscroll-behavior:\s*none;/s,
    );
    expect(ruleFor('.app-viewport #app')).toMatch(/height:\s*100%;[^}]*overflow:\s*hidden;/s);
    expect(ruleFor('.app-viewport > .layout-main')).toMatch(
      /flex:\s*1 1 auto;[^}]*min-height:\s*0;[^}]*overflow:\s*hidden;/s,
    );
  });

  it('keeps new-conversation scrolling inside its main area without a nested viewport minimum', () => {
    const pageRule = ruleFor('.new-conv-page');
    expect(pageRule).toMatch(/height:\s*100%;/);
    expect(pageRule).toMatch(/min-height:\s*0;/);
    expect(pageRule).toMatch(/overflow:\s*hidden;/);
    expect(pageRule).not.toMatch(/100(?:d)?vh/);

    const mainRule = ruleFor('.new-conv-main');
    expect(mainRule).toMatch(/min-height:\s*0;/);
    expect(mainRule).toMatch(/overflow:\s*auto;/);
  });
});
