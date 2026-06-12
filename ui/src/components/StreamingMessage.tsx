import React, { memo, useEffect, useMemo, useRef, useState } from 'react';
import { useTheme } from '../hooks/useTheme';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import type { StreamingBuffer } from '../conversation/atom';
import { useStreamingBuffer } from '../conversation/useConversationAtom';
import { parseStreamingBlocks, type StreamingBlock } from '../utils/parseStreamingBlocks';
import { ConversationMarkdownAnchor } from './conversationMarkdown';

// Stable markdown configuration — avoids creating new references on every render
const REMARK_PLUGINS = [remarkGfm];

type MarkdownTableProps = React.ComponentPropsWithoutRef<'table'> & { node?: unknown };

function MarkdownTable({ node, children, ...props }: MarkdownTableProps) {
  void node;
  return (
    <div className="markdown-table-scroll">
      <table {...props}>{children}</table>
    </div>
  );
}

const MARKDOWN_COMPONENTS = {
  a: ConversationMarkdownAnchor,
  table: MarkdownTable,
};

/**
 * Subscribes to the streaming buffer for the given slug and renders it.
 * The subscription happens at this leaf so per-token atom mutations
 * re-render only this component — `<MessageList>` and its
 * `<MessageListBody>` are untouched by token churn (REQ-MLRU-010).
 */
export function StreamingMessage({ slug }: { slug: string }) {
  const buffer = useStreamingBuffer(slug);
  return <StreamingMessageView buffer={buffer} />;
}

interface StreamingMessageViewProps {
  buffer: StreamingBuffer | null;
}

/**
 * Renders a streaming buffer. Pure: takes the buffer as a prop. Use
 * `<StreamingMessage slug={...} />` for the subscribing wrapper;
 * `<StreamingMessageView />` is exported for tests that exercise the
 * rendering pipeline directly.
 *
 * Renders while the LLM is generating. When the final `sse_message` event arrives,
 * the reducer clears this buffer and appends the finalized message atomically — one
 * React render, no flicker, no duplication.
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
export function StreamingMessageView({ buffer }: StreamingMessageViewProps) {
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;
  // rAF-gated display buffer: accumulates incoming text and flushes once per frame.
  const pendingText = useRef<string>('');
  const rafHandle = useRef<number | null>(null);
  const [displayText, setDisplayText] = useState<string>('');

  const incomingText = buffer?.text ?? '';

  // Update the pending text and schedule a single rAF flush per frame.
  // The previous cleanup-on-every-dep-change pattern cancelled in-flight
  // frames on every token, defeating the coalescing intent — display
  // would freeze during a burst and only catch up on idle. The
  // scheduled rAF below is allowed to fire; subsequent token effects
  // only update pendingText and skip re-scheduling because rafHandle
  // is non-null.
  useEffect(() => {
    pendingText.current = incomingText;
    if (rafHandle.current === null) {
      rafHandle.current = requestAnimationFrame(() => {
        setDisplayText(pendingText.current);
        rafHandle.current = null;
      });
    }
  }, [incomingText]);

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
  const blocks = useMemo(() => parseStreamingBlocks(displayText), [displayText]);

  if (!buffer) return null;

  return (
    <div className="streaming-message agent-message">
      <div className="streaming-message-content">
        {blocks.map((block, i) => (
          <StreamingBlock key={`${block.type}-${i}`} block={block} syntaxStyle={syntaxStyle} />
        ))}
      </div>
      <span className="streaming-cursor" aria-hidden="true" />
    </div>
  );
}

type StreamingBlockProps = {
  block: StreamingBlock;
  syntaxStyle: Record<string, React.CSSProperties>;
};

// Each parse returns fresh block objects, so referential `memo` would never
// hit. A completed block is immutable by construction — its rendered output
// is fully determined by (type, content, complete, lang) + the syntax theme.
// Comparing those fields lets every block except the growing tail skip the
// per-frame ReactMarkdown re-parse / Prism re-highlight during streaming.
function streamingBlocksEqual(prev: StreamingBlockProps, next: StreamingBlockProps): boolean {
  if (prev.syntaxStyle !== next.syntaxStyle) return false;
  const a = prev.block;
  const b = next.block;
  if (a.type !== b.type || a.content !== b.content) return false;
  if (a.type === 'code' && b.type === 'code') {
    return a.complete === b.complete && a.lang === b.lang;
  }
  return true;
}

const StreamingBlock = memo(function StreamingBlock({ block, syntaxStyle }: StreamingBlockProps) {
  if (block.type === 'markdown') {
    return (
      <div className="agent-text-block">
        <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={MARKDOWN_COMPONENTS}>
          {block.content}
        </ReactMarkdown>
      </div>
    );
  }

  if (block.complete) {
    return (
      <SyntaxHighlighter
        style={syntaxStyle}
        language={block.lang || 'text'}
        PreTag="div"
      >
        {block.content.replace(/\n$/, '')}
      </SyntaxHighlighter>
    );
  }

  // Incomplete (open) code block: render as plain monospace in a container
  // that matches SyntaxHighlighter's dimensions exactly. When the closing
  // fence arrives and complete flips to true, only colors change — no reflow.
  return (
    <pre className="streaming-code-pre">
      <code className="streaming-code">{block.content}</code>
    </pre>
  );
}, streamingBlocksEqual);
