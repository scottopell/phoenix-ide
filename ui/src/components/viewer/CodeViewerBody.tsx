import type { createElementProps } from '../../utils/syntaxHighlighter';
import { SyntaxHighlighter, createElement, oneDark } from '../../utils/syntaxHighlighter';
import { AnnotatableBlock } from './AnnotatableBlock';
import type { ViewerBodyProps } from './AnnotatableBlock';

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
}: CodeViewerBodyProps) {
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
                    {createElement({ node, stylesheet, useInlineStyles, key: `t-${idx}` } as createElementProps)}
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
