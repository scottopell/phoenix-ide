// useToast.tsx
import { useState, useCallback } from 'react';
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

  return {
    toasts,
    showToast,
    dismissToast,
    // Convenience methods
    showInfo: (message: string, duration?: number) => showToast('info', message, duration),
    showWarning: (message: string, duration?: number) => showToast('warning', message, duration),
    showError: (message: string, duration?: number) => showToast('error', message, duration),
    showSuccess: (message: string, duration?: number) => showToast('success', message, duration),
  };
}