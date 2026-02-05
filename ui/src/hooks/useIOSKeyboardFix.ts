import { useEffect, useRef, useCallback } from 'react';

/**
 * iOS Safari keyboard fix using visualViewport API.
 * 
 * Problem: With position:fixed on html/body (to prevent overscroll), iOS Safari
 * doesn't resize the layout viewport when the keyboard opens. The #app container
 * stays at full window height (100dvh) while the visual viewport shrinks,
 * leaving the input area hidden under the keyboard.
 * 
 * Solution: Listen to visualViewport resize events and dynamically set #app height
 * to match the visual viewport height when keyboard is open.
 */
export function useIOSKeyboardFix() {
  const isIOSRef = useRef<boolean | null>(null);
  const originalHeightRef = useRef<string | null>(null);
  const lastVVHeight = useRef<number>(0);

  const debugLog = (msg: string, data?: unknown) => {
    if (typeof window !== 'undefined' && (window as { __iosKbDebug?: boolean }).__iosKbDebug) {
      console.log(`[iOS-KB] ${msg}`, data ?? '');
    }
  };

  const updateAppHeight = useCallback(() => {
    const vv = window.visualViewport;
    const app = document.getElementById('app');
    if (!vv || !app) return;

    // Calculate keyboard height
    const keyboardHeight = window.innerHeight - vv.height;
    const isKeyboardOpen = keyboardHeight > 100; // Threshold to detect keyboard vs URL bar

    debugLog('updateAppHeight', {
      windowInnerHeight: window.innerHeight,
      vvHeight: vv.height,
      keyboardHeight,
      isKeyboardOpen,
      currentAppHeight: app.style.height,
    });

    if (isKeyboardOpen) {
      // Keyboard is open - set #app height to visual viewport height
      const targetHeight = vv.height;
      app.style.height = `${targetHeight}px`;
      debugLog('Set #app height to', targetHeight);
      
      // Ensure window doesn't scroll
      if (window.scrollY !== 0) {
        window.scrollTo(0, 0);
        debugLog('Reset window scroll');
      }

      // Also scroll the focused input into view within the #app container
      // This helps if the main-area needs to scroll to show the input
      const activeEl = document.activeElement;
      if (activeEl instanceof HTMLInputElement || activeEl instanceof HTMLTextAreaElement) {
        // Small delay to let layout settle
        setTimeout(() => {
          activeEl.scrollIntoView({ block: 'nearest', behavior: 'instant' });
        }, 50);
      }
    } else {
      // Keyboard closed - reset to CSS default
      if (app.style.height) {
        app.style.height = '';
        debugLog('Reset #app height to CSS default');
      }
    }

    lastVVHeight.current = vv.height;
  }, []);

  useEffect(() => {
    // Check if we're on iOS Safari
    if (isIOSRef.current === null) {
      const ua = navigator.userAgent;
      isIOSRef.current = /iPad|iPhone|iPod/.test(ua) && !('MSStream' in window);
      debugLog('iOS detection', { isIOS: isIOSRef.current, ua });
    }
    
    if (!isIOSRef.current) return;

    const vv = window.visualViewport;
    if (!vv) {
      debugLog('No visualViewport API');
      return;
    }

    // Store original height
    const app = document.getElementById('app');
    if (app) {
      originalHeightRef.current = app.style.height;
    }

    debugLog('Setting up listeners');
    lastVVHeight.current = vv.height;

    // Listen for visualViewport resize
    vv.addEventListener('resize', updateAppHeight);
    vv.addEventListener('scroll', updateAppHeight);

    // Also handle window scroll to keep it at 0 when keyboard is open
    const handleScroll = () => {
      const keyboardOpen = vv.height < window.innerHeight - 100;
      if (keyboardOpen && window.scrollY !== 0) {
        window.scrollTo(0, 0);
      }
    };
    window.addEventListener('scroll', handleScroll, { passive: true });

    // Handle focus events to trigger layout update when input is focused
    const handleFocusIn = (e: FocusEvent) => {
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        // Delay to let iOS finish its scroll/resize
        setTimeout(updateAppHeight, 100);
        setTimeout(updateAppHeight, 300);
      }
    };
    document.addEventListener('focusin', handleFocusIn);

    // Initial check
    updateAppHeight();

    return () => {
      debugLog('Cleaning up listeners');
      vv.removeEventListener('resize', updateAppHeight);
      vv.removeEventListener('scroll', updateAppHeight);
      window.removeEventListener('scroll', handleScroll);
      document.removeEventListener('focusin', handleFocusIn);
      
      // Reset height on cleanup
      if (app) {
        app.style.height = originalHeightRef.current || '';
      }
    };
  }, [updateAppHeight]);
}
