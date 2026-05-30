import { AnnotatableBlock } from './AnnotatableBlock';
import type { ViewerBodyProps } from './AnnotatableBlock';

/**
 * Plain-text body: line-numbered, annotatable, no syntax highlighting. The
 * fallback render kind for files the classifier does not recognize as
 * markdown, code, or html.
 */
export function TextViewerBody({
  content,
  modifiedLines,
  highlightedLine,
  onAnnotate,
  registerLineRef,
}: ViewerBodyProps) {
  const lines = content.split('\n');
  return (
    <div className="prose-reader-text">
      {lines.map((line, index) => {
        const lineNumber = index + 1;
        return (
          <AnnotatableBlock
            key={lineNumber}
            lineNumber={lineNumber}
            lineContent={line}
            onAnnotate={onAnnotate}
            className="prose-line"
            isModified={modifiedLines.has(lineNumber)}
            isHighlighted={highlightedLine === lineNumber}
            lineRef={(el) => registerLineRef(lineNumber, el)}
          >
            <span className="prose-line__number">{lineNumber}</span>
            <span className="prose-line__content">{line || ' '}</span>
          </AnnotatableBlock>
        );
      })}
    </div>
  );
}
