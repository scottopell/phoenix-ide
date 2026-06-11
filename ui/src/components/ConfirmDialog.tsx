import { useEffect, useRef } from 'react';

interface ConfirmDialogProps {
  visible: boolean;
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  visible,
  title,
  message,
  confirmText = 'Confirm',
  cancelText = 'Cancel',
  danger = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (visible) {
      const handleEscape = (e: KeyboardEvent) => {
        if (e.key === 'Escape') onCancel();
      };
      document.addEventListener('keydown', handleEscape);
      return () => document.removeEventListener('keydown', handleEscape);
    }
    return undefined;
  }, [visible, onCancel]);

  if (!visible) return null;

  return (
    <div
      className="modal-overlay"
      onClick={onCancel}
      title="Cancel and close dialog"
      aria-label="Cancel and close dialog"
    >
      <div
        ref={dialogRef}
        className="modal confirm-dialog"
        onClick={(e) => e.stopPropagation()}
        title={title}
      >
        <h3>{title}</h3>
        <p className="confirm-message">{message}</p>
        <div className="modal-actions">
          <button className="btn-secondary" onClick={onCancel} title={cancelText}>
            {cancelText}
          </button>
          <button
            className={danger ? 'btn-danger' : 'btn-primary'}
            onClick={onConfirm}
            title={danger ? `${confirmText} (can't be undone)` : confirmText}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}
