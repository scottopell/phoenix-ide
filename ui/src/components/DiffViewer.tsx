import { useEffect, useMemo } from 'react';

interface DiffViewerProps {
  open: boolean;
  comparator: string;
  commitLog: string;
  committedDiff: string;
  committedTruncatedKib?: number | undefined;
  uncommittedDiff: string;
  uncommittedTruncatedKib?: number | undefined;
  onClose: () => void;
}

/**
 * Modal overlay rendering a worktree diff vs base. Two sections —
 * committed-on-branch and uncommitted working-tree — each with simple
 * line-based coloring (green for additions, red for deletions, muted for
 * hunk headers). Esc and click-on-overlay close the modal.
 */
export function DiffViewer({
  open,
  comparator,
  commitLog,
  committedDiff,
  committedTruncatedKib,
  uncommittedDiff,
  uncommittedTruncatedKib,
  onClose,
}: DiffViewerProps) {
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      className="diff-viewer-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="diff-viewer-panel" role="dialog" aria-label="Worktree diff">
        <div className="diff-viewer-header">
          <div className="diff-viewer-title">
            Diff vs <code>{comparator}</code>
          </div>
          <button
            className="diff-viewer-close"
            onClick={onClose}
            aria-label="Close diff viewer"
          >
            ✕
          </button>
        </div>
        <div className="diff-viewer-body">
          {commitLog.trim() && (
            <DiffSection title="Commits" body={commitLog} kind="log" />
          )}
          {committedDiff.trim() ? (
            <DiffSection
              title="Committed changes"
              body={committedDiff}
              kind="diff"
              truncatedKib={committedTruncatedKib}
            />
          ) : (
            !commitLog.trim() && (
              <div className="diff-viewer-empty">
                No committed changes vs <code>{comparator}</code>.
              </div>
            )
          )}
          {uncommittedDiff.trim() && (
            <DiffSection
              title="Uncommitted changes"
              body={uncommittedDiff}
              kind="diff"
              truncatedKib={uncommittedTruncatedKib}
            />
          )}
        </div>
      </div>
    </div>
  );
}

interface DiffSectionProps {
  title: string;
  body: string;
  kind: 'diff' | 'log';
  truncatedKib?: number | undefined;
}

function DiffSection({ title, body, kind, truncatedKib }: DiffSectionProps) {
  const lines = useMemo(() => body.split('\n'), [body]);
  return (
    <section className="diff-section">
      <h3 className="diff-section-title">
        {title}
        {truncatedKib !== undefined && (
          <span className="diff-section-truncated">
            (truncated; {truncatedKib} KiB total)
          </span>
        )}
      </h3>
      {/* `<div>` children inside `<pre>` is invalid HTML — browsers may
          implicitly close the `<pre>` and break whitespace/layout. Use a
          regular div with white-space:pre (set in CSS) instead. */}
      <div className={`diff-pre diff-pre-${kind}`} role="region" aria-label={title}>
        {lines.map((line, i) => (
          <div key={i} className={kind === 'diff' ? lineClass(line) : 'diff-line'}>
            {line || ' '}
          </div>
        ))}
      </div>
    </section>
  );
}

function lineClass(line: string): string {
  // `diff --git`, `index `, `---`, `+++` are file headers — bolder display.
  if (
    line.startsWith('diff --git') ||
    line.startsWith('index ') ||
    line.startsWith('--- ') ||
    line.startsWith('+++ ')
  ) {
    return 'diff-line diff-file-header';
  }
  if (line.startsWith('@@')) return 'diff-line diff-hunk';
  if (line.startsWith('+')) return 'diff-line diff-add';
  if (line.startsWith('-')) return 'diff-line diff-del';
  return 'diff-line diff-context';
}
