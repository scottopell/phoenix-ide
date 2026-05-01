import { ChevronDown, Send, Trash2 } from 'lucide-react';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';

interface NotesPanelProps {
  notes: ReviewNote[];
  /** Click on a note jumps to its anchor in the viewer. Optional —
   *  callers without scroll-into-view targets can omit. */
  onJumpTo?: ((note: ReviewNote) => void) | undefined;
  onRemove: (id: string) => void;
  onClearAll: () => void;
  onSend: () => void;
  onClose: () => void;
}

/**
 * Sidebar panel listing pending review notes. Shared between FileView
 * and DiffView. Each entry shows the anchor label, the quoted source
 * line, and the user's note body, with a delete button. The panel
 * footer has Clear All / Send All actions.
 *
 * Send drops the entire pile into the chat input via the parent's
 * `onSend` callback (which formats via `formatNotesForSend`).
 */
export function NotesPanel({
  notes,
  onJumpTo,
  onRemove,
  onClearAll,
  onSend,
  onClose,
}: NotesPanelProps) {
  return (
    <div className="notes-panel">
      <div className="notes-panel-header">
        <span>Notes ({notes.length})</span>
        <button onClick={onClose} aria-label="Hide notes panel">
          <ChevronDown size={18} />
        </button>
      </div>
      <div className="notes-panel-list">
        {notes.map((n) => (
          <div key={n.id} className="notes-panel-note">
            <div className="notes-panel-note-header">
              <button
                className="notes-panel-note-anchor"
                onClick={() => onJumpTo?.(n)}
                disabled={!onJumpTo}
              >
                {anchorLabel(n)}
              </button>
              <button
                className="notes-panel-note-delete"
                onClick={() => onRemove(n.id)}
                aria-label="Delete note"
              >
                <Trash2 size={14} />
              </button>
            </div>
            {n.lineContent && (
              <div className="notes-panel-note-preview">
                {n.lineContent.slice(0, 60)}
                {n.lineContent.length > 60 && '…'}
              </div>
            )}
            <div className="notes-panel-note-text">{n.body}</div>
          </div>
        ))}
      </div>
      <div className="notes-panel-actions">
        <button onClick={onClearAll}>Clear All</button>
        <button className="primary" onClick={onSend}>
          <Send size={16} />
          Send All
        </button>
      </div>
    </div>
  );
}

function anchorLabel(n: ReviewNote): string {
  switch (n.anchor.kind) {
    case 'file':
      return `Line ${n.anchor.lineNumber}`;
    case 'diff':
      if (n.anchor.newLine !== undefined) return `New line ${n.anchor.newLine}`;
      if (n.anchor.oldLine !== undefined) return `Removed line ${n.anchor.oldLine}`;
      return `Diff position ${n.anchor.diffPos}`;
    case 'diff-file':
      return 'File-level';
  }
}
