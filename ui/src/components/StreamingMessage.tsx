import { useEffect, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { StreamingBuffer } from '../conversation/atom';
import { parseStreamingBlocks, type StreamingBlock } from '../utils/parseStreamingBlocks';

// Stable plugin array — avoids creating a new array reference on every render
const REMARK_PLUGINS = [remarkGfm];

interface StreamingMessageProps {
  buffer: StreamingBuffer | null;
}

/**
 * Displays in-progress streaming text below the message list (REQ-UI-019, REQ-BED-025).
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
export function StreamingMessage({ buffer }: StreamingMessageProps) {
  // rAF-gated display buffer: accumulates incoming text and flushes once per frame.
  const pendingText = useRef<string>('');
  const rafHandle = useRef<number | null>(null);
  const [displayText, setDisplayText] = useState<string>('');

  const incomingText = buffer?.text ?? '';

  useEffect(() => {
    pendingText.current = incomingText;

    if (rafHandle.current === null) {
      rafHandle.current = requestAnimationFrame(() => {
        setDisplayText(pendingText.current);
        rafHandle.current = null;
      });
    }

    return () => {
      if (rafHandle.current !== null) {
        cancelAnimationFrame(rafHandle.current);
        rafHandle.current = null;
      }
    };
  }, [incomingText]);

  if (!buffer) return null;

  const blocks = parseStreamingBlocks(displayText);

  return (
    <div className="streaming-message agent-message">
      <div className="streaming-message-content">
        {blocks.map((block, i) => (
          <StreamingBlock key={`${block.type}-${i}`} block={block} />
        ))}
      </div>
      <span className="streaming-cursor" aria-hidden="true" />
    </div>
  );
}

function StreamingBlock({ block }: { block: StreamingBlock }) {
  if (block.type === 'markdown') {
    return (
      <div className="agent-text-block">
        <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
          {block.content}
        </ReactMarkdown>
      </div>
    );
  }

  if (block.complete) {
    return (
      <SyntaxHighlighter
        style={oneDark}
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
}
