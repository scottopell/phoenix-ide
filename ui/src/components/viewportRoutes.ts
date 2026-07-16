import { useLayoutEffect } from 'react';

export function isViewportOwnedRoute(pathname: string, desktop: boolean): boolean {
  return (desktop && /^\/$/.test(pathname))
    || /^\/new\/?$/.test(pathname)
    || /^\/c\/[^/]+\/?$/.test(pathname)
    || /^\/global(?:\/[^/]+)?\/?$/.test(pathname);
}

export function useDocumentViewportOwnership(ownsViewport: boolean): void {
  useLayoutEffect(() => {
    document.documentElement.classList.toggle('app-viewport-active', ownsViewport);
    document.body.classList.toggle('app-viewport-active', ownsViewport);

    return () => {
      document.documentElement.classList.remove('app-viewport-active');
      document.body.classList.remove('app-viewport-active');
    };
  }, [ownsViewport]);
}

function canScrollInDirection(element: HTMLElement, deltaY: number): boolean {
  const maxScrollTop = element.scrollHeight - element.clientHeight;
  if (maxScrollTop <= 0) return false;

  const overflowY = getComputedStyle(element).overflowY;
  const isScrollOwner = element.matches('[data-app-scroll-owner], textarea')
    || overflowY === 'auto'
    || overflowY === 'scroll';
  if (!isScrollOwner) return false;

  return deltaY > 0 ? element.scrollTop > 0 : element.scrollTop < maxScrollTop - 1;
}

function hasScrollableAncestor(target: Element | null, deltaY: number): boolean {
  let element = target;
  while (element) {
    if (element instanceof HTMLElement && canScrollInDirection(element, deltaY)) return true;
    element = element.parentElement;
  }
  return false;
}

export function useAppTouchContainment(ownsViewport: boolean): void {
  useLayoutEffect(() => {
    if (!ownsViewport) return;

    let lastX = 0;
    let lastY = 0;
    const onTouchStart = (event: TouchEvent) => {
      const touch = event.touches[0];
      if (!touch) return;
      lastX = touch.clientX;
      lastY = touch.clientY;
    };
    const onTouchMove = (event: TouchEvent) => {
      if (event.touches.length !== 1) return;
      const touch = event.touches[0];
      if (!touch) return;

      const deltaX = touch.clientX - lastX;
      const deltaY = touch.clientY - lastY;
      lastX = touch.clientX;
      lastY = touch.clientY;
      if (Math.abs(deltaY) <= Math.abs(deltaX)) return;

      const target = event.target instanceof Element ? event.target : null;
      if (!hasScrollableAncestor(target, deltaY)) event.preventDefault();
    };

    document.addEventListener('touchstart', onTouchStart, { passive: true });
    document.addEventListener('touchmove', onTouchMove, { passive: false });
    return () => {
      document.removeEventListener('touchstart', onTouchStart);
      document.removeEventListener('touchmove', onTouchMove);
    };
  }, [ownsViewport]);
}
