import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';

interface AnnotationDialogProps {
  /** Header label, e.g. "Line 42" or "src/foo.rs (file-level)". */
  anchorLabel: string;
  /** Source line being annotated; truncated for the preview. Empty for
   *  file-level. */
  lineContent: string;
  /** Initial textarea value (when editing an existing note). */
  initialBody?: string;
  onSubmit: (body: string) => void;
  onCancel: () => void;
}

/**
 * Modal-within-the-viewer for entering a note body. Esc cancels;
 * Cmd/Ctrl+Enter submits. Click on the dim overlay also cancels.
 *
 * Shared between FileView and DiffView so the dialog is identical
 * regardless of anchor kind.
 */
export function AnnotationDialog({
  anchorLabel,
  lineContent,
  initialBody = '',
  onSubmit,
  onCancel,
}: AnnotationDialogProps) {
  const [body, setBody] = useState(initialBody);
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    taRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onCancel();
      } else if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        if (body.trim()) {
          e.preventDefault();
          onSubmit(body.trim());
        }
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [body, onCancel, onSubmit]);

  return (
    <div
      className="annotation-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="annotation-dialog">
        <div className="annotation-dialog-header">
          <span>{anchorLabel}</span>
          <button onClick={onCancel} aria-label="Cancel note">
            <X size={18} />
          </button>
        </div>
        {lineContent && (
          <div className="annotation-dialog-preview">
            {lineContent.slice(0, 100)}
            {lineContent.length > 100 && '…'}
          </div>
        )}
        <textarea
          ref={taRef}
          className="annotation-dialog-input"
          placeholder="Add your note… (Cmd/Ctrl+Enter to save)"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          rows={3}
        />
        <div className="annotation-dialog-actions">
          <button onClick={onCancel}>Cancel</button>
          <button
            className="primary"
            onClick={() => body.trim() && onSubmit(body.trim())}
            disabled={!body.trim()}
          >
            {initialBody ? 'Save' : 'Add Note'}
          </button>
        </div>
      </div>
    </div>
  );
}
