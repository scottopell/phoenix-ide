// Toast.tsx
import { useEffect, useState } from 'react';
import './Toast.css';

export type ToastType = 'info' | 'warning' | 'error' | 'success';

export interface ToastMessage {
  id: string;
  type: ToastType;
  message: string;
  duration: number | undefined;
}

interface ToastProps {
  messages: ToastMessage[];
  onDismiss: (id: string) => void;
}

export function Toast({ messages, onDismiss }: ToastProps) {
  return (
    <div className="toast-container">
      {messages.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

function ToastItem({ toast, onDismiss }: { toast: ToastMessage; onDismiss: (id: string) => void }) {
  const [isLeaving, setIsLeaving] = useState(false);

  useEffect(() => {
    if (toast.duration !== 0) {
      const timer = setTimeout(() => {
        setIsLeaving(true);
        setTimeout(() => onDismiss(toast.id), 300);
      }, toast.duration ?? 5000);

      return () => clearTimeout(timer);
    }
    return undefined;
  }, [toast, onDismiss]);

  const handleDismiss = () => {
    setIsLeaving(true);
    setTimeout(() => onDismiss(toast.id), 300);
  };

  return (
    <div 
      className={`toast toast-${toast.type} ${isLeaving ? 'toast-leaving' : ''}`}
      onClick={handleDismiss}
    >
      <div className="toast-icon">
        {toast.type === 'warning' && '⚠️'}
        {toast.type === 'error' && '❌'}
        {toast.type === 'success' && '✅'}
        {toast.type === 'info' && 'ℹ️'}
      </div>
      <div className="toast-message">{toast.message}</div>
    </div>
  );
}