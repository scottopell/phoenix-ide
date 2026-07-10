import { useLayoutEffect } from 'react';

export function isViewportOwnedRoute(pathname: string): boolean {
  return /^\/new\/?$/.test(pathname) || /^\/c\/[^/]+\/?$/.test(pathname);
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
