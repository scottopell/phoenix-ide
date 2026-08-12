import { useEffect, useId, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import './SelectionDialog.css';

interface SelectionDialogProps {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  dismissible?: boolean;
  restoreFocusRef?: React.RefObject<HTMLElement | null>;
  ariaBusy?: boolean;
  className?: string;
}

export function SelectionDialog({
  title,
  description,
  children,
  footer,
  onClose,
  dismissible = true,
  restoreFocusRef,
  ariaBusy = false,
  className = '',
}: SelectionDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const previousFocusRef = useRef<HTMLElement | null>(
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  );

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const restoreTarget = restoreFocusRef?.current ?? previousFocusRef.current;

    if (typeof dialog.showModal === 'function') {
      dialog.showModal();
    } else {
      dialog.setAttribute('open', '');
    }

    const firstFocus = dialog.querySelector<HTMLElement>('[data-selection-dialog-autofocus]');
    (firstFocus ?? dialog).focus({ preventScroll: true });

    return () => {
      if (dialog.open && typeof dialog.close === 'function') dialog.close();
      requestAnimationFrame(() => {
        if (restoreTarget && document.contains(restoreTarget)) {
          restoreTarget.focus({ preventScroll: true });
        }
      });
    };
  }, [restoreFocusRef]);

  const requestClose = () => {
    if (dismissible) onClose();
  };

  return createPortal(
    <dialog
      ref={dialogRef}
      className={`selection-dialog ${className}`.trim()}
      aria-labelledby={titleId}
      aria-describedby={description ? descriptionId : undefined}
      aria-busy={ariaBusy || undefined}
      onCancel={(event) => {
        event.preventDefault();
        requestClose();
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          requestClose();
        }
      }}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div className="selection-dialog__surface">
        <header className="selection-dialog__header">
          <div className="selection-dialog__heading">
            <h2 id={titleId}>{title}</h2>
            {description ? <div id={descriptionId} className="selection-dialog__description">{description}</div> : null}
          </div>
          <button
            type="button"
            className="selection-dialog__close"
            aria-label={`Close ${title}`}
            onClick={requestClose}
            disabled={!dismissible}
          >
            ×
          </button>
        </header>
        <div className="selection-dialog__body">{children}</div>
        {footer ? <footer className="selection-dialog__footer">{footer}</footer> : null}
      </div>
    </dialog>,
    document.body,
  );
}
