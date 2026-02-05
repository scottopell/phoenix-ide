import { useEffect, useRef } from 'react';

/**
 * iOS Safari keyboard scroll fix.
 * 
 * Problem: With viewport-fit=cover, iOS Safari doesn't resize the viewport when
 * the keyboard opens. Instead, it scrolls the window to keep the focused input
 * visible. This can scroll the entire #app container off screen.
 * 
 * Solution: When an input is focused, immediately scroll the window back to (0,0)
 * and let our flexbox layout handle the viewport sizing via 100dvh.
 */
export function useIOSKeyboardFix() {
  const isIOSRef = useRef<boolean | null>(null);

  useEffect(() => {
    // Check if we're on iOS Safari
    if (isIOSRef.current === null) {
      const ua = navigator.userAgent;
      isIOSRef.current = /iPad|iPhone|iPod/.test(ua) && !('MSStream' in window);
    }
    
    if (!isIOSRef.current) return;

    // Function to reset window scroll position
    const resetWindowScroll = () => {
      if (window.scrollY !== 0 || window.scrollX !== 0) {
        window.scrollTo(0, 0);
      }
    };

    // Handle focus events - when input is focused, iOS may scroll
    const handleFocusIn = (e: FocusEvent) => {
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        // Slight delay to let iOS do its scroll, then reset
        setTimeout(resetWindowScroll, 50);
        setTimeout(resetWindowScroll, 100);
        setTimeout(resetWindowScroll, 200);
      }
    };

    // Also listen for visualViewport resize events
    const handleViewportResize = () => {
      // When keyboard opens/closes, reset window scroll
      resetWindowScroll();
    };

    // Listen for window scroll and immediately reset if needed
    const handleScroll = () => {
      // Only reset if an input is focused (keyboard is likely open)
      const active = document.activeElement;
      if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) {
        resetWindowScroll();
      }
    };

    document.addEventListener('focusin', handleFocusIn);
    window.addEventListener('scroll', handleScroll, { passive: true });
    
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', handleViewportResize);
    }

    return () => {
      document.removeEventListener('focusin', handleFocusIn);
      window.removeEventListener('scroll', handleScroll);
      if (vv) {
        vv.removeEventListener('resize', handleViewportResize);
      }
    };
  }, []);
}
