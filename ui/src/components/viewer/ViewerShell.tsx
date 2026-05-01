import { useEffect } from 'react';
import { ArrowLeft, MessageSquare, Send } from 'lucide-react';
import type { ReactNode } from 'react';

export type ViewerMode = 'overlay' | 'inline';

interface ViewerShellProps {
  mode: ViewerMode;
  /** ARIA label for the dialog/region — used by screen readers and
   *  test queries (e.g. `getByRole('dialog', { name: 'Worktree diff' })`). */
  ariaLabel: string;
  /** Header title — file name, "Diff vs <base>", etc. */
  title: ReactNode;
  /** Tooltip on the title (e.g. absolute path). */
  titleTooltip?: string | undefined;
  /** Right-side actions (mode toggles, etc.) rendered before the
   *  notes badge / send button. */
  headerExtras?: ReactNode;
  /** Number of pending review notes for THIS viewer's scope; drives
   *  badge and send-button visibility. */
  noteCount: number;
  /** Toggle the notes side panel. The panel itself is rendered by the
   *  caller via `panel` so each viewer owns its own scroll/jump logic. */
  onToggleNotes: () => void;
  /** Send the entire review-notes pile and clear it. Called from the
   *  header send button. */
  onSend: () => void;
  /** Optional banner shown below the header (e.g. "viewing N changes
   *  from patch"). */
  banner?: ReactNode;
  onClose: () => void;
  /** Main content — file render, diff lines, etc. */
  children: ReactNode;
  /** Notes side panel rendered absolutely over the body; caller
   *  controls visibility. */
  panel?: ReactNode;
  /** Annotation dialog (note entry). Caller-rendered for the same
   *  reason. */
  dialog?: ReactNode;
  /** Confirmation dialog (e.g. unsaved-notes-on-close). */
  confirm?: ReactNode;
}

/**
 * Shared chrome for content-viewer modals. Used by FileView (formerly
 * ProseReader) and DiffView (formerly DiffViewer's body). Handles the
 * overlay / inline mode switch, header layout, Esc-to-close (which
 * defers to the caller-supplied `onClose`, which may show a confirm),
 * and slots for the body / notes panel / annotation dialog.
 *
 * `mode="overlay"` — fixed full-screen modal with backdrop. Today's
 * default for both viewers.
 * `mode="inline"` — pure flex item with no overlay. Used by the
 * desktop split-pane layout (task 08654) so the viewer can sit beside
 * the chat instead of taking it over.
 */
export function ViewerShell({
  mode,
  ariaLabel,
  title,
  titleTooltip,
  headerExtras,
  noteCount,
  onToggleNotes,
  onSend,
  banner,
  onClose,
  children,
  panel,
  dialog,
  confirm,
}: ViewerShellProps) {
  // Esc closes (deferring to caller — they may guard with a confirm).
  // Capture phase + stopPropagation so this shell catches Esc before
  // outer focus-scope handlers steal it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // If a dialog or confirm is rendered, let those handle Esc first.
      if (dialog || confirm) return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, dialog, confirm]);

  return (
    <div
      className={mode === 'overlay' ? 'viewer-shell viewer-shell--overlay' : 'viewer-shell viewer-shell--inline'}
      role={mode === 'overlay' ? 'dialog' : 'region'}
      aria-label={ariaLabel}
      aria-modal={mode === 'overlay' ? true : undefined}
    >
      <div className="viewer-shell-header">
        <button
          className="viewer-shell-btn"
          onClick={onClose}
          aria-label="Close viewer"
        >
          <ArrowLeft size={20} />
        </button>
        <div className="viewer-shell-title" title={titleTooltip}>
          {title}
        </div>
        <div className="viewer-shell-actions">
          {headerExtras}
          {noteCount > 0 && (
            <>
              <button
                className="viewer-shell-badge"
                onClick={onToggleNotes}
                aria-label={`${noteCount} notes`}
              >
                <MessageSquare size={18} />
                <span>{noteCount}</span>
              </button>
              <button
                className="viewer-shell-send-btn"
                onClick={onSend}
                aria-label="Send notes"
              >
                <Send size={18} />
              </button>
            </>
          )}
        </div>
      </div>
      {banner && <div className="viewer-shell-banner">{banner}</div>}
      <div className="viewer-shell-body">{children}</div>
      {panel}
      {dialog}
      {confirm}
    </div>
  );
}
