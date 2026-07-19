import { Children, cloneElement, isValidElement, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter } from '../../utils/syntaxHighlighter';
import { useTheme } from '../../hooks/useTheme';
import { oneDark, oneLight } from '../../utils/syntaxHighlighter';
import { AnnotatableBlock } from './AnnotatableBlock';
import type { ViewerBodyProps } from './AnnotatableBlock';
import { MermaidDiagram } from '../MermaidDiagram';
import { buildMarkdownFileSearchProjection } from '../viewer-find';

// Stable plugin reference — a fresh array each render defeats ReactMarkdown's
// internal memoization and forces a full re-parse.
const REMARK_PLUGINS = [remarkGfm];

function decorateMarkdownFindChildren(
  children: React.ReactNode,
  matches: readonly { start: number; end: number; occurrenceIndex: number }[],
  activeOccurrence: number,
): React.ReactNode {
  let cursor = 0;
  const decorate = (node: React.ReactNode): React.ReactNode => Children.map(node, (child) => {
    if (typeof child === 'string') {
      const childStart = cursor;
      cursor += child.length;
      const fragments: React.ReactNode[] = [];
      let localCursor = 0;
      for (const match of matches.filter((candidate) => candidate.start < cursor && candidate.end > childStart)) {
        const start = Math.max(localCursor, match.start - childStart);
        const end = Math.min(child.length, match.end - childStart);
        if (start > localCursor) fragments.push(child.slice(localCursor, start));
        if (end > start) fragments.push(
          <mark
            key={`${match.start}-${match.end}-${match.occurrenceIndex}`}
            className={match.occurrenceIndex === activeOccurrence ? 'viewer-find-match viewer-find-match--active' : 'viewer-find-match'}
            data-find-occurrence={match.occurrenceIndex}
          >
            {child.slice(start, end)}
          </mark>,
        );
        localCursor = Math.max(localCursor, end);
      }
      if (localCursor < child.length) fragments.push(child.slice(localCursor));
      return fragments.length > 0 ? fragments : child;
    }
    if (!isValidElement<{ children?: React.ReactNode }>(child)) return child;
    return cloneElement(child, {}, decorate(child.props.children));
  });
  return decorate(children);
}

/**
 * Rendered-markdown body with per-block annotation. Each block element (p, h1-3,
 * list item, table cell, blockquote) is wrapped so a long-press anchors a note
 * to the block's start line; fenced code is syntax-highlighted with the active
 * theme.
 */
export function MarkdownViewerBody({
  content,
  modifiedLines,
  highlightedLine,
  onAnnotate,
  registerLineRef,
  findQuery = '',
  activeFindOccurrence = null,
}: ViewerBodyProps) {
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;
  const findProjection = useMemo(
    () => findQuery ? buildMarkdownFileSearchProjection(content, findQuery) : { sources: [], matches: [] },
    [content, findQuery],
  );
  const matchesBySourceOffset = useMemo(() => {
    const matches = new Map<number, Array<{ start: number; end: number; occurrenceIndex: number }>>();
    findProjection.matches.forEach((match, occurrenceIndex) => {
      const sourceOffset = Number(/:(\d+)-\d+$/.exec(match.sourceId)?.[1]);
      if (!Number.isFinite(sourceOffset)) return;
      const blockMatches = matches.get(sourceOffset) ?? [];
      blockMatches.push({ start: match.start, end: match.end, occurrenceIndex });
      matches.set(sourceOffset, blockMatches);
    });
    return matches;
  }, [findProjection.matches]);

  // Recompute only when content/handlers/highlight state actually change — a
  // fresh `components` map (or `annotatable` factory) every render would force
  // every block element and every fenced-code highlight to re-render.
  const components = useMemo(() => {
    const rawLines = content.split('\n');

    const annotatable = (Tag: React.ElementType) =>
      ({ children, node, ...props }: { children?: React.ReactNode; node?: { position?: { start?: { line?: number; offset?: number }; end?: { line?: number } } }; [key: string]: unknown }) => {
        const ln = node?.position?.start?.line ?? 0;
        const startLine = (node?.position?.start?.line ?? 1) - 1;
        const endLine = (node?.position?.end?.line ?? startLine + 1) - 1;
        const rawLineContent = rawLines.slice(startLine, endLine + 1).join(' ').slice(0, 200);
        const sourceOffset = node?.position?.start?.offset;
        const blockMatches = sourceOffset === undefined ? [] : matchesBySourceOffset.get(sourceOffset) ?? [];
        return (
          <AnnotatableBlock
            as={Tag}
            lineNumber={ln}
            lineContent={rawLineContent}
            onAnnotate={onAnnotate}
            className="viewer-markdown-block"
            isModified={modifiedLines.has(ln)}
            isHighlighted={highlightedLine === ln}
            lineRef={(el) => registerLineRef(ln, el)}
            {...props}
          >
            {blockMatches.length > 0
              ? decorateMarkdownFindChildren(children, blockMatches, activeFindOccurrence ?? -1)
              : children}
          </AnnotatableBlock>
        );
      };
    return {
      p: annotatable('p'),
      h1: annotatable('h1'),
      h2: annotatable('h2'),
      h3: annotatable('h3'),
      h4: annotatable('h4'),
      h5: annotatable('h5'),
      h6: annotatable('h6'),
      td: annotatable('td'),
      th: annotatable('th'),
      li: annotatable('li'),
      blockquote: annotatable('blockquote'),
      code: ({ inline, className, children, ...props }: { inline?: boolean; className?: string; children?: React.ReactNode; [key: string]: unknown }) => {
        const match = /language-([^\s]+)/.exec(className || '');
        const language = match?.[1]?.toLowerCase();
        if (!inline && language === 'mermaid') {
          return <MermaidDiagram code={String(children)} />;
        }
        return !inline && match ? (
          <SyntaxHighlighter style={syntaxStyle} language={match[1]} PreTag="div" {...props}>
            {String(children).replace(/\n$/, '')}
          </SyntaxHighlighter>
        ) : (
          <code className={className} {...props}>{children}</code>
        );
      },
    } as unknown as Components;
  }, [activeFindOccurrence, content, highlightedLine, matchesBySourceOffset, modifiedLines, onAnnotate, registerLineRef, syntaxStyle]);

  return (
    <div className="viewer-markdown">
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={components}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
