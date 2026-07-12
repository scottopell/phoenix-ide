import { useEffect, useId, useRef } from 'react';
import './FindBar.css';

export interface FindBarProps {
  query: string;
  activeIndex: number;
  matchCount: number;
  onQueryChange: (query: string) => void;
  onNext: () => void;
  onPrevious: () => void;
  onClose: () => void;
  autoFocus?: boolean;
  focusVersion?: number;
}

export function FindBar({
  query,
  activeIndex,
  matchCount,
  onQueryChange,
  onNext,
  onPrevious,
  onClose,
  autoFocus = true,
  focusVersion = 0,
}: FindBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const labelId = useId();
  const statusId = useId();

  useEffect(() => {
    if (!autoFocus) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [autoFocus, focusVersion]);

  const statusText = matchCount === 0 ? '0 results' : `${activeIndex + 1} of ${matchCount}`;

  return (
    <div className="viewer-find-bar" role="search" aria-labelledby={labelId}>
      <label id={labelId} className="viewer-find-bar__field">
        <span className="sr-only">Find in viewer</span>
        <input
          ref={inputRef}
          type="text"
          value={query}
          data-viewer-find-input="true"
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={(event) => {
            if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
              event.preventDefault();
              inputRef.current?.focus();
              inputRef.current?.select();
            } else if (event.key === 'Enter') {
              event.preventDefault();
              if (event.shiftKey) onPrevious();
              else onNext();
            } else if (event.key === 'Escape') {
              event.preventDefault();
              onClose();
            }
          }}
          aria-describedby={statusId}
          aria-label="Find in viewer"
          spellCheck={false}
        />
      </label>
      <div id={statusId} className="viewer-find-bar__status" aria-live="polite">
        {statusText}
      </div>
      <button type="button" className="viewer-find-bar__button" onClick={onPrevious} disabled={matchCount === 0}>
        Previous
      </button>
      <button type="button" className="viewer-find-bar__button" onClick={onNext} disabled={matchCount === 0}>
        Next
      </button>
      <button type="button" className="viewer-find-bar__button" onClick={onClose}>
        Close
      </button>
    </div>
  );
}
