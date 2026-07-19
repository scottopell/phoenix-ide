import { useEffect } from 'react';
import { ArrowLeft, Maximize2, MessageSquare, Minimize2, Send } from 'lucide-react';
import type { ReactNode } from 'react';
import type { FocusedReviewExitTarget } from './useFocusedReviewExit';

export type ViewerMode = 'overlay' | 'inline' | 'takeover';

interface ViewerShellProps {
  /** When false, shell-level Escape handling stands down so an inner surface
   *  (for example viewer find) can consume Escape first. */
  closeOnEscape?: boolean | undefined;
  /** Called when Escape should dismiss inner viewer chrome (for example an open
   *  find bar) before the shell itself closes. */
  onInnerEscape?: (() => void) | undefined;
  /** When true, suppresses the close button's default mousedown focus transfer so
   *  nested affordances can restore focus to their own opener instead. */
  suppressCloseButtonFocus?: boolean | undefined;
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
  /** Optional Escape action when it differs from the header close action. */
  onEscape?: (() => void) | undefined;
  /** Scroll strategy for the shell body. The default lets file viewers scroll
   *  their `.viewer-content`; `children` lets virtualized children such as the
   *  diff CodeView own wheel/trackpad scrolling without a competing parent. */
  bodyScroll?: 'shell' | 'children' | undefined;
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
 * `mode="takeover"` — fixed full-screen surface above app chrome. Used by
 * dismissible focused review surfaces such as fullscreen diff.
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
  onEscape,
  children,
  panel,
  dialog,
  confirm,
  bodyScroll = 'shell',
  closeOnEscape = true,
  onInnerEscape,
  suppressCloseButtonFocus = false,
}: ViewerShellProps) {
  // Esc closes (deferring to caller — they may guard with a confirm).
  // Registered in capture phase with stopPropagation so this shell
  // catches Esc before outer focus-scope handlers can swallow it. The
  // matching removeEventListener must use the same `capture` flag.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // If a dialog/confirm is rendered, let the inner surface own Esc first.
      if (dialog || confirm) return;
      if (!closeOnEscape) {
        if (!onInnerEscape) return;
        e.stopPropagation();
        onInnerEscape();
        return;
      }
      e.stopPropagation();
      (onEscape ?? onClose)();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose, onEscape, dialog, confirm, closeOnEscape, onInnerEscape]);

  const modal = mode !== 'inline';
  const className = mode === 'inline'
    ? 'viewer-shell viewer-shell--inline'
    : mode === 'takeover'
      ? 'viewer-shell viewer-shell--overlay viewer-shell--takeover'
      : 'viewer-shell viewer-shell--overlay';

  return (
    <div
      className={className}
      role={modal ? 'dialog' : 'region'}
      aria-label={ariaLabel}
      aria-modal={modal ? true : undefined}
    >
      <div className="viewer-shell-header">
        <button
          className="viewer-shell-btn"
          onMouseDown={suppressCloseButtonFocus ? (event) => event.preventDefault() : undefined}
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
      <div className={`viewer-shell-body viewer-shell-body--scroll-${bodyScroll}`}>{children}</div>
      {panel}
      {dialog}
      {confirm}
    </div>
  );
}

export function ViewerPresentationControl({
  fullscreen,
  onToggle,
}: {
  fullscreen: boolean;
  onToggle: () => void;
}) {
  const label = fullscreen ? 'Return to pane' : 'Fullscreen';
  return (
    <button
      type="button"
      className="viewer-shell-toggle viewer-shell-icon-toggle"
      onClick={onToggle}
      aria-label={label}
      title={label}
    >
      {fullscreen ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
      <span>{label}</span>
    </button>
  );
}

export function FocusedReviewExitDialog({
  target,
  sending,
  error,
  onSend,
  onDiscard,
  onKeepReviewing,
}: {
  target: FocusedReviewExitTarget;
  sending: boolean;
  error: string | null;
  onSend: () => void;
  onDiscard: () => void;
  onKeepReviewing: () => void;
}) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || sending) return;
      event.stopPropagation();
      onKeepReviewing();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onKeepReviewing, sending]);
  const closing = target === 'closed';
  const title = closing ? 'Resolve feedback before closing' : 'Resolve feedback before returning';
  const destination = closing ? 'closing the viewer' : 'returning to the pane';
  const sendLabel = closing ? 'Send feedback and close' : 'Send feedback and return';
  const discardLabel = closing ? 'Discard notes and close' : 'Discard notes and return';
  return (
    <div className="focused-review-exit-overlay" role="presentation">
      <div className="modal confirm-dialog focused-review-exit" role="dialog" aria-modal="true" aria-labelledby="focused-review-exit-title">
        <h3 id="focused-review-exit-title">{title}</h3>
        <p>Pending notes belong to this full-screen review. Send or discard them before {destination}.</p>
        {error && <p className="viewer-send-error" role="alert">{error}</p>}
        <div className="modal-actions focused-review-exit-actions">
          <button className="btn-secondary" type="button" onClick={onKeepReviewing} disabled={sending}>Keep reviewing</button>
          <button className="btn-danger" type="button" onClick={onDiscard} disabled={sending}>{discardLabel}</button>
          <button className="btn-primary" type="button" onClick={onSend} disabled={sending}>
            {sending ? 'Sending…' : sendLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
