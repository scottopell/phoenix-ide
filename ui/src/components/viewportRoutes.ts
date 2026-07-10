import { useLayoutEffect, type RefObject } from 'react';

export function isViewportOwnedRoute(pathname: string, desktop: boolean): boolean {
  return (desktop && /^\/$/.test(pathname))
    || /^\/new\/?$/.test(pathname)
    || /^\/c\/[^/]+\/?$/.test(pathname);
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

export function useAppTouchContainment(
  ownerRef: RefObject<HTMLElement>,
  ownsViewport: boolean,
): void {
  useLayoutEffect(() => {
    const owner = ownerRef.current;
    if (!ownsViewport || !owner) return;

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
      const scrollOwner = target?.closest<HTMLElement>('[data-app-scroll-owner]');
      if (!scrollOwner || !owner.contains(scrollOwner)) {
        event.preventDefault();
        return;
      }

      const maxScrollTop = scrollOwner.scrollHeight - scrollOwner.clientHeight;
      const pullingPastTop = deltaY > 0 && scrollOwner.scrollTop <= 0;
      const pushingPastBottom = deltaY < 0 && scrollOwner.scrollTop >= maxScrollTop - 1;
      if (pullingPastTop || pushingPastBottom) event.preventDefault();
    };

    owner.addEventListener('touchstart', onTouchStart, { passive: true });
    owner.addEventListener('touchmove', onTouchMove, { passive: false });
    return () => {
      owner.removeEventListener('touchstart', onTouchStart);
      owner.removeEventListener('touchmove', onTouchMove);
    };
  }, [ownerRef, ownsViewport]);
}
