import { useState, useEffect } from 'react';

/**
 * Detects when the mobile keyboard is likely visible.
 * 
 * Since viewport-fit=cover causes iOS to resize the viewport rather than
 * report keyboard height via visualViewport, we detect keyboard by checking
 * if an input/textarea element is focused.
 */
export function useKeyboardVisible(): boolean {
  const [isKeyboardVisible, setIsKeyboardVisible] = useState(false);

  useEffect(() => {
    const checkFocus = () => {
      const activeElement = document.activeElement;
      const isInputFocused = activeElement instanceof HTMLInputElement ||
                             activeElement instanceof HTMLTextAreaElement;
      setIsKeyboardVisible(isInputFocused);
    };

    // Check on focus/blur events
    document.addEventListener('focusin', checkFocus);
    document.addEventListener('focusout', checkFocus);
    
    // Initial check
    checkFocus();

    return () => {
      document.removeEventListener('focusin', checkFocus);
      document.removeEventListener('focusout', checkFocus);
    };
  }, []);

  return isKeyboardVisible;
}
