import { AnnotatableBlock } from './AnnotatableBlock';
import type { ViewerBodyProps } from './AnnotatableBlock';
import type { TextRenderMode } from './metaViewerTypes';

interface TextViewerBodyProps extends ViewerBodyProps {
  mode?: TextRenderMode | undefined;
}

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
  mode = 'rich',
}: TextViewerBodyProps) {
  if (mode === 'plainLargeText') {
    return (
      <div className="viewer-text" data-testid="viewer-large-text-fallback">
        <pre className="viewer-large-text-pre">{content}</pre>
      </div>
    );
  }

  const lines = content.split('\n');
  return (
    <div className="viewer-text">
      {lines.map((line, index) => {
        const lineNumber = index + 1;
        return (
          <AnnotatableBlock
            key={lineNumber}
            lineNumber={lineNumber}
            lineContent={line}
            onAnnotate={onAnnotate}
            className="viewer-text-line"
            isModified={modifiedLines.has(lineNumber)}
            isHighlighted={highlightedLine === lineNumber}
            lineRef={(el) => registerLineRef(lineNumber, el)}
          >
            <span className="viewer-text-line__number">{lineNumber}</span>
            <span className="viewer-text-line__content">{line || ' '}</span>
          </AnnotatableBlock>
        );
      })}
    </div>
  );
}
