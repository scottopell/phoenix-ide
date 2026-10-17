import React, { memo, useEffect, useMemo, useRef, useState } from 'react';
import { useTheme } from '../hooks/useTheme';
import ReactMarkdown from 'react-markdown';
import type { Components, ExtraProps } from 'react-markdown';
import type { Code } from 'mdast';
import { MarkdownRenderer } from './MarkdownRenderer';
import { prepareStreamingBlocks, type PreparedStreamingBlock } from './prepareStreamingBlocks';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import type { StreamingBuffer } from '../conversation/atom';
import { useStreamingBuffer } from '../conversation/useConversationAtom';
import { ConversationMarkdownAnchor, ConversationMarkdownImage } from './conversationMarkdown';
import { ConversationMarkdownTable } from './conversationMarkdownTable';
import { inlineCodeTokenKind } from './conversationMarkdownTableSemantics';
import { CONVERSATION_MARKDOWN_URL_TRANSFORM, resolveConversationMarkdownImageSrc } from './conversationMarkdownImages';
import { MermaidDiagram } from './MermaidDiagram';
import { formatMessageTime } from './MessageComponents';
import { AssistantMessageChrome, AssistantMessageMetadata } from './AssistantMessageChrome';

type StreamingMarkdownComponentsContext = {
  rootDir?: string | undefined;
  onFileClick?: ((filePath: string) => void) | undefined;
};

function createStreamingMarkdownComponents({ rootDir, onFileClick }: StreamingMarkdownComponentsContext) {
  return {
    a: (props: React.ComponentPropsWithoutRef<'a'> & { node?: unknown }) => (
      <ConversationMarkdownAnchor
        {...props}
        onFileClick={onFileClick}
        filePathCopyContext={rootDir ? { rootDir } : undefined}
      />
    ),
    code: ({ children, node, ...props }: React.ComponentPropsWithoutRef<'code'> & ExtraProps) => {
      const language = /language-([^\s]+)/.exec(props.className ?? '')?.[1];
      const complete = node?.properties['data-streaming-complete'];
      if (typeof complete === 'boolean') return <StreamingCode content={String(children).replace(/\n$/, '')} lang={String(node?.properties['data-streaming-lang'] ?? '')} complete={complete} />;
      if (language?.toLowerCase() === 'mermaid') return <MemoMermaid code={String(children)} />;
      if (language) return <DisclosureCode language={language}>{String(children)}</DisclosureCode>;
      return <code {...props} data-token-kind={inlineCodeTokenKind(children)}>{children}</code>;
    },
    pre: ({ children, node, ...props }) => {
      const code = node?.children[0];
      if (code?.type === 'element' && typeof code.properties['data-streaming-complete'] === 'boolean') return <>{children}</>;
      return <pre {...props}>{children}</pre>;
    },
    table: ConversationMarkdownTable,
    img: ({ src, ...props }: React.ComponentPropsWithoutRef<'img'> & { node?: unknown }) => (
      <ConversationMarkdownImage
        {...props}
        src={resolveConversationMarkdownImageSrc(src, rootDir)}
      />
    ),
  } satisfies Components;
}

const MemoMermaid = memo(MermaidDiagram);

const DisclosureCode = memo(function DisclosureCode({ language, children }: { language: string; children: string }) {
  const { theme } = useTheme();
  return <SyntaxHighlighter style={theme === 'light' ? oneLight : oneDark} language={language} PreTag="div">{children.replace(/\n$/, '')}</SyntaxHighlighter>;
});

const MARKDOWN_COMPONENTS = createStreamingMarkdownComponents({});

/**
 * Subscribes to the streaming buffer for the given slug and renders it.
 * The subscription happens at this leaf so per-token atom mutations
 * re-render only this component — `<MessageList>` and its
 * `<MessageListBody>` are untouched by token churn (REQ-MLRU-010).
 */
export function StreamingMessage({
  slug,
  requestId,
  isFirstInTurn,
  rootDir,
  onOpenFile,
}: {
  slug: string;
  requestId: string;
  isFirstInTurn: boolean;
  rootDir?: string | undefined;
  onOpenFile?: ((filePath: string) => void) | undefined;
}) {
  const buffer = useStreamingBuffer(slug);
  return <StreamingMessageView buffer={buffer?.requestId === requestId ? buffer : null} isFirstInTurn={isFirstInTurn} rootDir={rootDir} onOpenFile={onOpenFile} />;
}

interface StreamingMessageViewProps {
  buffer: StreamingBuffer | null;
  // Render the Phoenix/timestamp header iff this stream is the turn's first agent
  // message, so the header row is present during streaming and does not pop in
  // (shifting content down) on finalize. Defaults true for tests/standalone use.
  isFirstInTurn?: boolean;
  rootDir?: string | undefined;
  onOpenFile?: ((filePath: string) => void) | undefined;
}

/**
 * Renders a streaming buffer. Pure: takes the buffer as a prop. Use
 * `<StreamingMessage slug={...} />` for the subscribing wrapper;
 * `<StreamingMessageView />` is exported for tests that exercise the
 * rendering pipeline directly.
 *
 * Uses rAF gating (not debouncing) to coalesce tokens that arrive within a single
 * 16ms frame, capping renders at ~60fps without introducing artificial latency.
 *
 * Renders blocks progressively:
 * - Prose → ReactMarkdown (bold, links, headings, etc.)
 * - Complete code fence → SyntaxHighlighter with Prism
 * - Open code fence → <pre><code className="streaming-code"> with matching
 *   dimensions so the swap to SyntaxHighlighter causes no layout shift.
 */
export function StreamingMessageView({ buffer, isFirstInTurn = true, rootDir, onOpenFile }: StreamingMessageViewProps) {
  if (!buffer) return null;
  const startedAtIso = new Date(buffer.startedAt).toISOString();
  return (
    <div className="message agent assistant-message">
      <AssistantMessageChrome
        isFirstInTurn={isFirstInTurn}
        metadata={<AssistantMessageMetadata timestamp={startedAtIso} formattedTime={formatMessageTime(startedAtIso)} />}
      />
      <div className="message-content">
        <StreamingBlocks text={buffer.text ?? ''} rootDir={rootDir} onOpenFile={onOpenFile} />
      </div>
      <span className="streaming-cursor" aria-hidden="true" />
    </div>
  );
}

/**
 * rAF-gated, block-memoized renderer for a live streaming text buffer. Owns the
 * coalescing + parse-once machinery so any surface that displays a growing token
 * buffer (the main agent stream, a sub-agent transcript) re-highlights only the
 * growing tail rather than re-parsing the whole buffer every token (REQ-MLRU-010).
 * Caller supplies the wrapping chrome (cursor, container classes).
 */
export function StreamingBlocks({
  text,
  rootDir,
  onOpenFile,
}: {
  text: string;
  rootDir?: string | undefined;
  onOpenFile?: ((filePath: string) => void) | undefined;
}) {
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;
  // rAF-gated display buffer: accumulates incoming text and flushes once per frame.
  const pendingText = useRef<string>('');
  const rafHandle = useRef<number | null>(null);
  const [displayText, setDisplayText] = useState<string>('');

  // Update the pending text and schedule a single rAF flush per frame.
  // The previous cleanup-on-every-dep-change pattern cancelled in-flight
  // frames on every token, defeating the coalescing intent — display
  // would freeze during a burst and only catch up on idle. The
  // scheduled rAF below is allowed to fire; subsequent token effects
  // only update pendingText and skip re-scheduling because rafHandle
  // is non-null.
  useEffect(() => {
    pendingText.current = text;
    if (rafHandle.current === null) {
      rafHandle.current = requestAnimationFrame(() => {
        setDisplayText(pendingText.current);
        rafHandle.current = null;
      });
    }
  }, [text]);

  // Cancel any pending frame on unmount only — not on every token.
  useEffect(() => {
    return () => {
      if (rafHandle.current !== null) {
        cancelAnimationFrame(rafHandle.current);
        rafHandle.current = null;
      }
    };
  }, []);

  // Parse once per text change, not on theme-only re-renders.
  const blocks = useMemo(() => prepareStreamingBlocks(displayText), [displayText]);
  const markdownComponents = useMemo(
    () => (rootDir || onOpenFile
      ? createStreamingMarkdownComponents({ rootDir, onFileClick: onOpenFile })
      : MARKDOWN_COMPONENTS),
    [rootDir, onOpenFile],
  );

  return (
    <>
      {blocks.map((block, i) => (
        <StreamingBlock key={`${block.type}-${i}`} block={block} syntaxStyle={syntaxStyle} markdownComponents={markdownComponents} />
      ))}
    </>
  );
}

type StreamingBlockProps = {
  block: PreparedStreamingBlock;
  syntaxStyle: Record<string, React.CSSProperties>;
  markdownComponents: React.ComponentProps<typeof ReactMarkdown>['components'];
};

// Canonical subtree revisions include changes to resolved global references.
function streamingBlocksEqual(prev: StreamingBlockProps, next: StreamingBlockProps): boolean {
  if (prev.syntaxStyle !== next.syntaxStyle || prev.markdownComponents !== next.markdownComponents) return false;
  const a = prev.block;
  const b = next.block;
  if (a.type !== b.type || a.content !== b.content || a.revision !== b.revision) return false;
  if (a.type === 'code' && b.type === 'code') {
    return a.complete === b.complete && a.lang === b.lang && a.meta === b.meta;
  }
  return true;
}

const StreamingBlock = memo(function StreamingBlock({ block, markdownComponents }: StreamingBlockProps) {
  if (block.type === 'markdown') {
    return (
      <div className="agent-text-block">
        <MarkdownRenderer mode="conversation" tree={block.tree} components={markdownComponents} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>
          {block.content}
        </MarkdownRenderer>
      </div>
    );
  }

  return <StreamingCode content={block.content} lang={block.lang} complete={block.complete} />;
}, streamingBlocksEqual);

const StreamingCode = memo(function StreamingCode({ content, lang, complete }: {
  content: string;
  lang: Code['lang'];
  complete: boolean;
}) {
  const { theme } = useTheme();
  if (!complete) return <pre className="streaming-code-pre"><code className="streaming-code">{content}</code></pre>;
  if (lang?.toLowerCase() === 'mermaid') return <MemoMermaid code={content} />;
  return <SyntaxHighlighter style={theme === 'light' ? oneLight : oneDark} language={lang || 'text'} PreTag="div">{content}</SyntaxHighlighter>;
});
