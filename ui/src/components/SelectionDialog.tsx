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

function resolveRestoreTarget(
  restoreFocusRef: React.RefObject<HTMLElement | null> | undefined,
  initialRestoreTarget: HTMLElement | null,
): HTMLElement | null {
  const currentRestoreTarget = restoreFocusRef?.current;
  return currentRestoreTarget && document.contains(currentRestoreTarget)
    ? currentRestoreTarget
    : initialRestoreTarget;
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
  const fallbackModalRef = useRef(false);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const initialRestoreTarget = restoreFocusRef?.current ?? previousFocusRef.current;

    const inertSiblings: Array<{ element: HTMLElement; wasInert: boolean }> = [];
    if (typeof dialog.showModal === 'function') {
      dialog.showModal();
    } else {
      fallbackModalRef.current = true;
      dialog.setAttribute('open', '');
      dialog.dataset['fallbackModal'] = '';
      for (const child of Array.from(document.body.children)) {
        if (!(child instanceof HTMLElement) || child === dialog) continue;
        inertSiblings.push({ element: child, wasInert: child.inert });
        child.inert = true;
      }
    }

    const firstFocus = dialog.querySelector<HTMLElement>('[data-selection-dialog-autofocus]');
    (firstFocus ?? dialog).focus({ preventScroll: true });

    return () => {
      if (dialog.open && typeof dialog.close === 'function') dialog.close();
      fallbackModalRef.current = false;
      for (const { element, wasInert } of inertSiblings) element.inert = wasInert;
      requestAnimationFrame(() => {
        const restoreTarget = resolveRestoreTarget(restoreFocusRef, initialRestoreTarget);
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
          return;
        }
        if (event.key !== 'Tab' || !fallbackModalRef.current) return;
        const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(
          'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
        ));
        if (focusable.length === 0) {
          event.preventDefault();
          event.currentTarget.focus();
          return;
        }
        const first = focusable[0]!;
        const last = focusable[focusable.length - 1]!;
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
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
