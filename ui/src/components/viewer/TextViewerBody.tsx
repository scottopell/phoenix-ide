import { useMemo } from 'react';
import type { ViewerBodyProps } from './AnnotatableBlock';
import { buildFileSearchProjection } from '../viewer-find';

/** Plain <pre> body for large source-like payloads that bypass rich rendering. */
export function TextViewerBody({ content, findQuery, activeFindOccurrence }: ViewerBodyProps) {
  const hasActiveQuery = (findQuery ?? '').length > 0 && activeFindOccurrence !== null && activeFindOccurrence !== undefined;
  const findProjection = useMemo(
    () => (hasActiveQuery ? buildFileSearchProjection(content, findQuery ?? '') : { sources: [], matches: [] }),
    [content, findQuery, hasActiveQuery],
  );
  const lineFragments = useMemo(() => {
    if (!hasActiveQuery) return null;
    const matchesByLine = new Map<number, Array<{ start: number; end: number; occurrenceIndex: number }>>();
    findProjection.matches.forEach((match, occurrenceIndex) => {
      const matches = matchesByLine.get(match.target.lineNumber) ?? [];
      matches.push({ start: match.start, end: match.end, occurrenceIndex });
      matchesByLine.set(match.target.lineNumber, matches);
    });

    return findProjection.sources.map((source) => ({
      lineNumber: source.lineNumber,
      fragments: renderFragments(source.text, matchesByLine.get(source.lineNumber) ?? [], activeFindOccurrence ?? -1),
    }));
  }, [activeFindOccurrence, findProjection.matches, findProjection.sources, hasActiveQuery]);

  return (
    <div className="viewer-text" data-testid="viewer-large-text-fallback" tabIndex={0}>
      <pre className="viewer-large-text-pre">
        {!hasActiveQuery || !lineFragments ? content : lineFragments.map(({ lineNumber, fragments }) => (
          <span key={lineNumber} data-find-line={lineNumber}>
            {fragments}
            {lineNumber < lineFragments.length ? '\n' : ''}
          </span>
        ))}
      </pre>
    </div>
  );
}

function renderFragments(
  text: string,
  matches: readonly { start: number; end: number; occurrenceIndex: number }[],
  activeOccurrence: number,
): React.ReactNode[] {
  if (matches.length === 0) return [text];
  const fragments: React.ReactNode[] = [];
  let cursor = 0;
  matches.forEach((match) => {
    const start = Math.max(match.start, cursor);
    const end = Math.max(match.end, start);
    if (start > cursor) fragments.push(text.slice(cursor, start));
    if (end > start) {
      fragments.push(
        <mark
          key={`${match.start}-${match.end}-${match.occurrenceIndex}`}
          className={match.occurrenceIndex === activeOccurrence ? 'viewer-find-match viewer-find-match--active' : 'viewer-find-match'}
          data-find-occurrence={match.occurrenceIndex}
        >
          {text.slice(start, end)}
        </mark>,
      );
    }
    cursor = Math.max(cursor, end);
  });
  if (cursor < text.length) fragments.push(text.slice(cursor));
  return fragments;
}
