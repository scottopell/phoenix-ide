import { useMemo } from 'react';
import type { ViewerBodyProps } from './AnnotatableBlock';
import { buildFileSearchProjection } from '../viewer-find';

/** Plain <pre> body for large source-like payloads that bypass rich rendering. */
export function TextViewerBody({ content, findQuery, activeFindOccurrence }: ViewerBodyProps) {
  const findProjection = useMemo(
    () => buildFileSearchProjection(content, findQuery ?? ''),
    [content, findQuery],
  );
  const lineFragments = useMemo(() => {
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
  }, [activeFindOccurrence, findProjection.matches, findProjection.sources]);

  return (
    <div className="viewer-text" data-testid="viewer-large-text-fallback">
      <pre className="viewer-large-text-pre">
        {lineFragments.map(({ lineNumber, fragments }) => (
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
    if (match.start > cursor) fragments.push(text.slice(cursor, match.start));
    fragments.push(
      <mark
        key={`${match.start}-${match.end}-${match.occurrenceIndex}`}
        className={match.occurrenceIndex === activeOccurrence ? 'viewer-find-match viewer-find-match--active' : 'viewer-find-match'}
        data-find-occurrence={match.occurrenceIndex}
      >
        {text.slice(match.start, match.end)}
      </mark>,
    );
    cursor = match.end;
  });
  if (cursor < text.length) fragments.push(text.slice(cursor));
  return fragments;
}
