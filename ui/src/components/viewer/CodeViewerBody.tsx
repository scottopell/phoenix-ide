import type { createElementProps } from '../../utils/syntaxHighlighter';
import { SyntaxHighlighter, createElement, oneDark } from '../../utils/syntaxHighlighter';
import { AnnotatableBlock } from './AnnotatableBlock';
import type { ViewerBodyProps } from './AnnotatableBlock';
import { buildFileSearchProjection } from '../viewer-find';

interface CodeViewerBodyProps extends ViewerBodyProps {
  language: string;
}

/**
 * Syntax-highlighted, line-numbered code rendering with per-line annotation.
 * Used for `code` payloads and for the source mode of `html` payloads.
 */
export function CodeViewerBody({
  content,
  language,
  modifiedLines,
  highlightedLine,
  onAnnotate,
  registerLineRef,
  findQuery = '',
  activeFindOccurrence = null,
}: CodeViewerBodyProps) {
  const findProjection = findQuery ? buildFileSearchProjection(content, findQuery) : { matches: [] };
  const matchesByLine = new Map<number, Array<{ start: number; end: number; occurrenceIndex: number }>>();
  findProjection.matches.forEach((match, occurrenceIndex) => {
    const matches = matchesByLine.get(match.target.lineNumber) ?? [];
    matches.push({ start: match.start, end: match.end, occurrenceIndex });
    matchesByLine.set(match.target.lineNumber, matches);
  });
  return (
    <div className="viewer-code">
      <SyntaxHighlighter
        style={oneDark}
        language={language}
        showLineNumbers
        renderer={({ rows, stylesheet, useInlineStyles }: { rows: unknown[]; stylesheet: unknown; useInlineStyles: boolean }) => {
          const lines = content?.split('\n') || [];
          return (
            <>
              {rows.map((node, idx) => {
                const lineNumber = idx + 1;
                return (
                  <AnnotatableBlock
                    key={lineNumber}
                    as="div"
                    lineNumber={lineNumber}
                    lineContent={lines[idx] || ''}
                    onAnnotate={onAnnotate}
                    className="viewer-code-line"
                    isModified={modifiedLines.has(lineNumber)}
                    isHighlighted={highlightedLine === lineNumber}
                    lineRef={(el) => registerLineRef(lineNumber, el)}
                  >
                    {matchesByLine.has(lineNumber)
                      ? renderCodeFindLine(lines[idx] ?? '', matchesByLine.get(lineNumber) ?? [], activeFindOccurrence ?? -1)
                      : createElement({ node, stylesheet, useInlineStyles, key: `t-${idx}` } as createElementProps)}
                  </AnnotatableBlock>
                );
              })}
            </>
          );
        }}
      >
        {content || ''}
      </SyntaxHighlighter>
    </div>
  );
}

function renderCodeFindLine(
  text: string,
  matches: readonly { start: number; end: number; occurrenceIndex: number }[],
  activeOccurrence: number,
) {
  const fragments: React.ReactNode[] = [];
  let cursor = 0;
  matches.forEach((match) => {
    if (match.start > cursor) fragments.push(text.slice(cursor, match.start));
    fragments.push(
      <mark
        key={match.occurrenceIndex}
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
