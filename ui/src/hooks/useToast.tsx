// useToast.tsx
import { useState, useCallback, useMemo } from 'react';
import type { ToastMessage, ToastType } from '../components/Toast';

export function useToast() {
  const [toasts, setToasts] = useState<ToastMessage[]>([]);

  const showToast = useCallback((type: ToastType, message: string, duration?: number) => {
    const id = `${Date.now()}-${Math.random()}`;
    const toast: ToastMessage = { id, type, message, duration };
    setToasts((prev) => [...prev, toast]);
  }, []);

  const dismissToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter(t => t.id !== id));
  }, []);

  // Memoize convenience methods to prevent re-renders causing dependency loops
  const showInfo = useCallback(
    (message: string, duration?: number) => showToast('info', message, duration),
    [showToast]
  );
  const showWarning = useCallback(
    (message: string, duration?: number) => showToast('warning', message, duration),
    [showToast]
  );
  const showError = useCallback(
    (message: string, duration?: number) => showToast('error', message, duration),
    [showToast]
  );
  const showSuccess = useCallback(
    (message: string, duration?: number) => showToast('success', message, duration),
    [showToast]
  );

  return useMemo(() => ({
    toasts,
    showToast,
    dismissToast,
    showInfo,
    showWarning,
    showError,
    showSuccess,
  }), [toasts, showToast, dismissToast, showInfo, showWarning, showError, showSuccess]);
}
