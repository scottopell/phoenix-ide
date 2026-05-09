import { useState, useEffect } from 'react';

/**
 * Subscribe to a CSS media query. Returns the current match state and
 * updates on viewport changes. Use the named breakpoint hooks below
 * unless you need an ad-hoc query.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, [query]);
  return matches;
}

// Named breakpoints — keep in sync with index.css. Centralised here so a
// browser resize across the boundary updates every consumer reactively
// instead of leaving stale `isDesktop` snapshots scattered through the tree.
export const useIsDesktop = () => useMediaQuery('(min-width: 1025px)');
export const useIsWideDesktop = () => useMediaQuery('(min-width: 1280px)');
export const useIsMobile = () => useMediaQuery('(max-width: 768px)');
