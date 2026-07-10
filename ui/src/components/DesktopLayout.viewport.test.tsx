import { readFileSync } from 'node:fs';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import {
  isViewportOwnedRoute,
  useAppTouchContainment,
  useDocumentViewportOwnership,
} from './viewportRoutes';

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
    (pathname) => expect(isViewportOwnedRoute(pathname, false)).toBe(true),
  );

  it('contains the desktop home composer but not the mobile conversation list', () => {
    expect(isViewportOwnedRoute('/', true)).toBe(true);
    expect(isViewportOwnedRoute('/', false)).toBe(false);
  });

  it.each(['/about', '/settings/llm-language', '/usage', '/terminal', '/s/token', '/c/', '/c/a/b'])(
    'keeps intentional document-scrolling route %s outside containment',
    (pathname) => expect(isViewportOwnedRoute(pathname, true)).toBe(false),
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

  it('blocks vertical touch chaining while preserving owned inner scrolling', () => {
    function Harness() {
      useAppTouchContainment(true);
      return (
        <div>
          <div data-testid="chrome" />
          <div data-testid="scroller" data-app-scroll-owner>
            <div data-testid="nested-scroller" style={{ overflowY: 'auto' }} />
          </div>
          <div data-testid="side-pane" style={{ overflowY: 'auto' }} />
          <textarea data-testid="textarea" />
        </div>
      );
    }

    const touch = (target: Element, ...ys: number[]) => {
      const event = new Event('touchmove', { bubbles: true, cancelable: true });
      Object.defineProperty(event, 'touches', {
        value: ys.map((clientY, index) => ({ clientX: 10 + index * 20, clientY })),
      });
      return target.dispatchEvent(event);
    };
    const start = (target: Element, y: number) => {
      const event = new Event('touchstart', { bubbles: true, cancelable: true });
      Object.defineProperty(event, 'touches', { value: [{ clientX: 10, clientY: y }] });
      target.dispatchEvent(event);
    };

    const { getByTestId } = render(<Harness />);
    const chrome = getByTestId('chrome');
    const scroller = getByTestId('scroller');
    const nestedScroller = getByTestId('nested-scroller');
    const sidePane = getByTestId('side-pane');
    const textarea = getByTestId('textarea');
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: 400 },
      scrollHeight: { configurable: true, value: 800 },
      scrollTop: { configurable: true, writable: true, value: 100 },
    });
    for (const element of [nestedScroller, sidePane, textarea]) {
      Object.defineProperties(element, {
        clientHeight: { configurable: true, value: 100 },
        scrollHeight: { configurable: true, value: 300 },
        scrollTop: { configurable: true, writable: true, value: 100 },
      });
    }

    start(sidePane, 100);
    expect(touch(sidePane, 80)).toBe(true);

    start(chrome, 100);
    expect(touch(chrome, 80, 120)).toBe(true);

    start(chrome, 100);
    expect(touch(chrome, 80)).toBe(false);

    start(scroller, 100);
    expect(touch(scroller, 80)).toBe(true);

    start(nestedScroller, 100);
    expect(touch(nestedScroller, 80)).toBe(true);

    nestedScroller.scrollTop = 200;
    start(nestedScroller, 100);
    expect(touch(nestedScroller, 80)).toBe(true);

    start(textarea, 100);
    expect(touch(textarea, 80)).toBe(true);

    scroller.scrollTop = 400;
    start(scroller, 100);
    expect(touch(scroller, 80)).toBe(false);

    start(nestedScroller, 100);
    expect(touch(nestedScroller, 80)).toBe(false);

    scroller.scrollTop = 0;
    start(scroller, 80);
    expect(touch(scroller, 100)).toBe(false);
  });

  it('contains dynamically mounted body portals while viewport ownership is active', () => {
    function Harness() {
      useAppTouchContainment(true);
      return null;
    }

    const { unmount } = render(<Harness />);
    const portal = document.createElement('div');
    document.body.append(portal);
    const start = new Event('touchstart', { bubbles: true, cancelable: true });
    Object.defineProperty(start, 'touches', { value: [{ clientX: 10, clientY: 100 }] });
    portal.dispatchEvent(start);
    const move = new Event('touchmove', { bubbles: true, cancelable: true });
    Object.defineProperty(move, 'touches', { value: [{ clientX: 10, clientY: 80 }] });
    expect(portal.dispatchEvent(move)).toBe(false);

    unmount();
    const moveAfterUnmount = new Event('touchmove', { bubbles: true, cancelable: true });
    Object.defineProperty(moveAfterUnmount, 'touches', { value: [{ clientX: 10, clientY: 80 }] });
    expect(portal.dispatchEvent(moveAfterUnmount)).toBe(true);
    portal.remove();
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
