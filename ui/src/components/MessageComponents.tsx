/**
 * Shared message rendering components used by both MessageList and VirtualizedMessageList.
 * 
 * IMPORTANT: Any UI changes to message rendering should be made HERE, not in the list
 * implementations. This ensures feature parity between regular and virtualized lists.
 * 
 * Components exported:
 * - UserMessage: Renders user messages with timestamps
 * - QueuedUserMessage: Renders pending/failed user messages
 * - AgentMessage: Renders agent responses with tool blocks
 * - ToolUseBlock: Renders individual tool use/result pairs
 * - SubAgentStatus: Renders sub-agent progress indicator
 */

import React, { memo, useState, useMemo, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import { api } from '../api';
import type { Message, ContentBlock, ToolResultContent, ConversationState, PendingSubAgent, SubAgentResult } from '../api';
import { cacheDB } from '../cache';
import type { QueuedMessage } from '../hooks';
import { useTheme } from '../hooks/useTheme';

import { linkifyText } from '../utils/linkify';
import { CopyButton } from './CopyButton';
import { PatchFileSummary, containsUnifiedDiff } from './PatchFileSummary';

const CheckIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);
const XIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <line x1="18" y1="6" x2="6" y2="18" />
    <line x1="6" y1="6" x2="18" y2="18" />
  </svg>
);
const ChevronDownIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="6 9 12 15 18 9" />
  </svg>
);
const ChevronRightIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="9 6 15 12 9 18" />
  </svg>
);
const ChevronUpIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="18 15 12 9 6 15" />
  </svg>
);

// Stable plugin array -- avoids creating a new array reference on every render
const REMARK_PLUGINS = [remarkGfm];

/** Format a tool execution duration for display in the tool block header.
 *  < 10s   -> "3.2s" (one decimal place)
 *  < 60s   -> "42s"
 *  >= 60s  -> "1m 4s" (seconds part omitted when 0: "2m")
 *
 *  Uses integer millisecond arithmetic so boundary values are exact:
 *  59 999 ms -> "59s", 60 000 ms -> "1m", 119 999 ms -> "1m 59s".
 */
function formatToolDuration(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  if (totalSeconds < 10) {
    return `${(ms / 1000).toFixed(1)}s`;
  }
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const m = Math.floor(totalSeconds / 60);
  const rem = totalSeconds % 60;
  return rem > 0 ? `${m}m ${rem}s` : `${m}m`;
}

// ============================================================================
// Helper functions
// ============================================================================

// eslint-disable-next-line react-refresh/only-export-components
export function formatMessageTime(isoStr: string): string {
  if (!isoStr) return '';
  const date = new Date(isoStr);
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();
  
  if (isToday) {
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  }
  return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

// Thresholds for auto-expanding output
const OUTPUT_AUTO_EXPAND_THRESHOLD = 200;  // Always show inline if under this

/**
 * Strip model artifacts from think tool thoughts:
 * - Remove optional opening <thinking> wrapper
 * - Truncate at </thinking> — everything after it is XML the model wrote
 *   as a narration of its next action (observed on claude-haiku-4-5-20251001).
 *   The actual tool call happens separately via the JSON API; this is just text.
 */
function cleanThoughts(raw: string): string {
  let text = raw.replace(/^\s*<thinking>\s*/i, '');
  const closingIdx = text.search(/<\/thinking>/i);
  if (closingIdx !== -1) {
    text = text.slice(0, closingIdx);
  }
  return text.trim();
}

function formatToolInput(name: string, input: Record<string, unknown>, displayOverride?: string): { display: string; isMultiline: boolean } {
  switch (name) {
    case 'bash': {
      const cmd = String(input['command'] || '');
      // Use server-provided display string if available (has cd prefix stripped)
      const displayCmd = displayOverride || cmd;
      return { display: `$ ${displayCmd}`, isMultiline: cmd.includes('\n') };
    }
    case 'think': {
      const thoughts = cleanThoughts(String(input['thoughts'] || ''));
      return { display: thoughts, isMultiline: thoughts.includes('\n') };
    }
    case 'patch': {
      const path = String(input['path'] || '');
      const patches = input['patches'] as Array<{ operation?: string }> | undefined;
      const op = patches?.[0]?.operation || 'modify';
      const count = patches?.length || 1;
      const summary = count > 1 ? `${path}: ${count} patches` : `${path}: ${op}`;
      return { display: summary, isMultiline: false };
    }
    case 'keyword_search': {
      const query = String(input['query'] || '');
      const terms = (input['search_terms'] as string[]) || [];
      const termsStr = terms.length > 0 ? terms.slice(0, 3).join(', ') + (terms.length > 3 ? '...' : '') : '';
      return { display: termsStr ? `"${query}" [${termsStr}]` : query, isMultiline: false };
    }
    case 'read_image': {
      const path = String(input['path'] || '');
      return { display: path, isMultiline: false };
    }
    case 'read_file': {
      const path = String(input['path'] || '');
      const offset = input['offset'] as number | undefined;
      const limit = input['limit'] as number | undefined;
      let display = path;
      if (offset !== undefined || limit !== undefined) {
        const start = offset ?? 1;
        const end = limit !== undefined ? start + limit - 1 : undefined;
        display = end !== undefined ? `${path}:${start}-${end}` : `${path}:${start}+`;
      }
      return { display, isMultiline: false };
    }
    case 'spawn_agents': {
      const tasks = (input['tasks'] as Array<{ task?: string }>) || [];
      const count = tasks.length;
      return {
        display: `${count} parallel task${count === 1 ? '' : 's'}`,
        isMultiline: false,
      };
    }
    case 'ask_user_question': {
      const questions = (input['questions'] as Array<{ question?: string; options?: unknown[] }>) || [];
      const first = questions[0];
      const rawText = String(first?.question || '');
      const flatText = rawText.replace(/\s+/g, ' ').trim();
      const truncated = flatText.length > 80 ? flatText.slice(0, 80) + '…' : flatText;
      const optionCount = Array.isArray(first?.options) ? first!.options!.length : 0;
      const suffix = questions.length > 1
        ? ` [+${questions.length - 1} more]`
        : optionCount > 0 ? ` [${optionCount} options]` : '';
      return { display: `"${truncated}"${suffix}`, isMultiline: false };
    }
    case 'search': {
      const pattern = String(input['pattern'] || '');
      const path = input['path'] ? String(input['path']) : '';
      const include = input['include'] ? String(input['include']) : '';
      let display = `"${pattern}"`;
      if (path) display += ` in ${path}`;
      if (include) display += ` (${include})`;
      return { display, isMultiline: false };
    }
    default: {
      if (name.startsWith('browser_')) {
        const display = formatBrowserInput(name, input);
        return { display, isMultiline: display.includes('\n') };
      }
      const str = JSON.stringify(input, null, 2);
      return { display: str, isMultiline: str.includes('\n') };
    }
  }
}

function truncateValue(s: string, max = 40): string {
  return s.length > max ? s.slice(0, max) + '…' : s;
}

function formatBrowserInput(name: string, input: Record<string, unknown>): string {
  switch (name) {
    case 'browser_navigate': {
      const url = String(input['url'] || '');
      return `→ ${url}`;
    }
    case 'browser_eval': {
      const expr = String(input['expression'] || '').replace(/\s+/g, ' ').trim();
      return `eval: ${truncateValue(expr, 80)}`;
    }
    case 'browser_take_screenshot': {
      const selector = input['selector'] ? String(input['selector']) : '';
      return selector ? `screenshot of "${selector}"` : 'screenshot';
    }
    case 'browser_recent_console_logs': {
      const limit = input['limit'] as number | undefined;
      return limit !== undefined ? `console logs (${limit})` : 'console logs';
    }
    case 'browser_clear_console_logs': {
      return 'clear console';
    }
    case 'browser_resize': {
      const width = input['width'];
      const height = input['height'];
      return `resize ${width}x${height}`;
    }
    case 'browser_wait_for_selector': {
      const selector = String(input['selector'] || '');
      const visible = input['visible'] === true;
      return visible ? `wait "${selector}" (visible)` : `wait "${selector}"`;
    }
    case 'browser_click': {
      const selector = String(input['selector'] || '');
      return `click "${selector}"`;
    }
    case 'browser_type': {
      const selector = String(input['selector'] || '');
      const text = String(input['text'] || '');
      const clear = input['clear'] === true;
      const verb = clear ? 'replace' : 'type';
      return `${verb} "${selector}" = "${truncateValue(text)}"`;
    }
    case 'browser_key_press': {
      const key = String(input['key'] || '');
      const modifiers = (input['modifiers'] as string[]) || [];
      const chord = modifiers.length > 0 ? `${modifiers.join('+')}+${key}` : key;
      return `key: ${chord}`;
    }
    default: {
      return JSON.stringify(input, null, 2);
    }
  }
}

// ============================================================================
// User Message Components
// ============================================================================

export const UserMessage = memo(UserMessageImpl);

function UserMessageImpl({ message }: { message: Message }) {
  const content = message.content as { text?: string; images?: { data: string; media_type: string }[]; is_meta?: boolean };
  const text = content.text || (typeof message.content === 'string' ? message.content : '');
  const images = content.images || [];
  const isMeta = content.is_meta === true;
  const timestamp = message.created_at;

  return (
    <div className={`message ${isMeta ? 'meta' : 'user'}`} data-sequence-id={message.sequence_id}>
      <div className="message-header">
        {!isMeta && <span className="message-sender">You</span>}
        {timestamp && (
          <span className="message-time" title={new Date(timestamp).toLocaleString()}>
            {formatMessageTime(timestamp)}
          </span>
        )}
        {!isMeta && <span className="message-status sent" title="Sent">&#x2713;</span>}
      </div>
      <div className="message-content">
        {text}
        {images.length > 0 && (
          <div className="message-images">
            {images.map((img, idx) => (
              <img
                key={idx}
                src={`data:${img.media_type};base64,${img.data}`}
                alt={`Attachment ${idx + 1}`}
                className="message-image"
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export const QueuedUserMessage = memo(QueuedUserMessageImpl);

// Pending user message: queued client-side, not yet echoed by the server.
// Failed-send messages render in InputArea, not here — this component assumes
// the entry it receives is pending (task 02676).
function QueuedUserMessageImpl({ message }: { message: QueuedMessage; onRetry: (localId: string) => void }) {
  return (
    <div className="message user">
      <div className="message-header">
        <span className="message-sender">You</span>
        <span className="message-status sending" title="Sending...">
          <span className="sending-spinner">⏳</span>
        </span>
      </div>
      <div className="message-content">
        {message.text}
        {message.images.length > 0 && (
          <div className="message-images">
            {message.images.map((img, idx) => (
              <img
                key={idx}
                src={`data:${img.media_type};base64,${img.data}`}
                alt={`Attachment ${idx + 1}`}
                className="message-image"
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ============================================================================
// Agent Message Components
// ============================================================================

interface AgentMessageProps {
  message: Message;
  toolResults: Map<string, Message>;
  onOpenFile?: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  /**
   * When false, suppresses the "Phoenix HH:MM" header row. Used by the list
   * to collapse repeated headers across a run of consecutive agent messages
   * within the same turn. Defaults to true so callers that don't set it keep
   * the original behavior.
   */
  isFirstInTurn?: boolean;
}

export const AgentMessage = memo(AgentMessageImpl);

function AgentMessageImpl({ message, toolResults, onOpenFile, isFirstInTurn = true }: AgentMessageProps) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const timestamp = message.created_at;
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;

  // Stable markdown component map — only recreated when onOpenFile identity changes.
  // Keeps ReactMarkdown from remounting SyntaxHighlighter on every parent re-render.
  const markdownComponents = useMemo(() => ({
    // Custom code block rendering with syntax highlighting
    // Inline code with file paths becomes clickable
    code: ({ inline, className, children, ...props }: { inline?: boolean | undefined; className?: string | undefined; children?: React.ReactNode }) => {
      const match = /language-(\w+)/.exec(className || '');
      if (!inline && match) {
        return (
          <SyntaxHighlighter
            style={syntaxStyle}
            language={match[1]}
            PreTag="div"
            {...props}
          >
            {String(children).replace(/\n$/, '')}
          </SyntaxHighlighter>
        );
      }
      // For inline code, check if it looks like a file path and make it clickable
      const text = String(children);
      const fileClickHandler = onOpenFile
        ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
        : undefined;
      const linkified = linkifyText(text, fileClickHandler);
      // If linkifyText returned something other than plain text, it found a file path
      if (linkified !== text && fileClickHandler) {
        return <>{linkified}</>;
      }
      return (
        <code className={className} {...props}>
          {children}
        </code>
      );
    },
    // Custom paragraph rendering with clickable file paths
    p: ({ children }: { children?: React.ReactNode }) => {
      const fileClickHandler = onOpenFile
        ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
        : undefined;
      const processChildren = (nodes: React.ReactNode): React.ReactNode[] => {
        return React.Children.toArray(nodes).flatMap((child) => {
          if (typeof child === 'string') {
            return linkifyText(child, fileClickHandler);
          }
          return child;
        });
      };
      return <p>{processChildren(children)}</p>;
    },
    // Custom list item rendering with clickable file paths
    li: ({ children }: { children?: React.ReactNode }) => {
      const fileClickHandler = onOpenFile
        ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
        : undefined;
      const processChildren = (nodes: React.ReactNode): React.ReactNode[] => {
        return React.Children.toArray(nodes).flatMap((child) => {
          if (typeof child === 'string') {
            return linkifyText(child, fileClickHandler);
          }
          return child;
        });
      };
      return <li>{processChildren(children)}</li>;
    },
  }), [onOpenFile, syntaxStyle]);

  // Check if there's any renderable content
  const hasRenderableContent = blocks.some(block => {
    if (block.type === 'text') {
      return block.text && block.text.trim() !== '';
    }
    if (block.type === 'tool_use') {
      return true;
    }
    return false;
  });

  // Don't render empty agent messages
  if (!hasRenderableContent) {
    return null;
  }

  return (
    <div className="message agent" data-sequence-id={message.sequence_id}>
      {isFirstInTurn && (
        <div className="message-header">
          <span className="message-sender">Phoenix</span>
          {timestamp && (
            <span className="message-time" title={new Date(timestamp).toLocaleString()}>
              {formatMessageTime(timestamp)}
            </span>
          )}
        </div>
      )}
      <div className="message-content">
        {blocks.map((block, i) => {
          if (block.type === 'text') {
            // Skip empty text blocks - they produce empty bubbles
            if (!block.text || block.text.trim() === '') {
              return null;
            }
            return (
              <div key={i} className="agent-text-block">
                <ReactMarkdown
                  remarkPlugins={REMARK_PLUGINS}
                  components={markdownComponents}
                >
                  {block.text}
                </ReactMarkdown>
              </div>
            );
          } else if (block.type === 'tool_use') {
            // `think` renders as a subtle inline aside, not the full tool-block
            // shell — it's model reasoning, not an action. Collapsed by default.
            if (block.name === 'think') {
              return <ThinkAside key={block.id || i} block={block} />;
            }
            return (
              <ToolUseBlock
                key={block.id || i}
                block={block}
                result={toolResults.get(block.id || '')}
                onOpenFile={onOpenFile}
              />
            );
          }
          return null;
        })}
      </div>
    </div>
  );
}

// ============================================================================
// Think Aside — subtle inline collapsed aside for `think` tool blocks
// ============================================================================

export const ThinkAside = memo(ThinkAsideImpl);

function ThinkAsideImpl({ block }: { block: ContentBlock }) {
  const input = (block.input || {}) as Record<string, unknown>;
  const raw = String(input['thoughts'] || '');
  const text = cleanThoughts(raw);
  const [expanded, setExpanded] = useState(false);

  // Empty thought after cleaning: render nothing.
  if (!text) return null;

  const lineCount = text.split('\n').length;

  return (
    <div className={`think-aside ${expanded ? 'expanded' : ''}`}>
      <div
        className="think-aside-header"
        onClick={() => setExpanded(!expanded)}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            setExpanded(!expanded);
          }
        }}
      >
        <span className="think-aside-chevron">
          {expanded ? <ChevronDownIcon /> : <ChevronRightIcon />}
        </span>
        <span className="think-aside-label">
          thinking ({lineCount} {lineCount === 1 ? 'line' : 'lines'})
        </span>
        {expanded && <CopyButton text={text} title="Copy thought" />}
      </div>
      {expanded && <div className="think-aside-body">{text}</div>}
    </div>
  );
}

// ============================================================================
// Tool Use Block
// ============================================================================

interface ToolUseBlockProps {
  block: ContentBlock;
  result: Message | undefined;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
}

// Helper to parse image data from read_image tool result
function parseImageResult(text: string): { media_type: string; data: string } | null {
  if (!text) return null;
  try {
    const parsed = JSON.parse(text);
    if (parsed.type === 'image' && parsed.media_type && parsed.data) {
      return { media_type: parsed.media_type, data: parsed.data };
    }
  } catch {
    // Not JSON or not an image result
  }
  return null;
}

export const ToolUseBlock = memo(ToolUseBlockImpl);

function ToolUseBlockImpl({ block, result, onOpenFile }: ToolUseBlockProps) {
  const name = block.name || 'tool';
  const input = block.input || {};
  const toolId = block.id || '';

  // Format the input display based on tool type
  // For bash, use server-provided display field (has cd prefix stripped)
  const { display: inputDisplay, isMultiline: inputIsMultiline } = formatToolInput(
    name,
    input as Record<string, unknown>,
    block.display
  );

  // Get the paired result if available
  let resultContent: ToolResultContent | null = null;
  if (result) {
    resultContent = result.content as ToolResultContent;
  }

  // Duration from display_data.duration_ms (set by Rust executor after tool completes)
  const durationMs: number | undefined = (() => {
    const dd = result?.display_data as Record<string, unknown> | undefined;
    const v = dd?.['duration_ms'];
    return typeof v === 'number' ? v : undefined;
  })();

  const rawResultText = resultContent?.content || resultContent?.result || resultContent?.error || '';
  const isError = resultContent?.is_error || !!resultContent?.error;
  
  // For patch tool, use the diff from display_data instead of the generic success message
  const patchDiff = name === 'patch' ? (result?.display_data as { diff?: string })?.diff : undefined;
  const resultText = patchDiff || rawResultText;
  const resultLength = resultText.length;
  
  // Check if this is an image result
  // First check display_data (preferred for browser_take_screenshot)
  // Then fall back to parsing the result content (for read_image)
  let imageResult: { media_type: string; data: string } | null = null;
  if (result?.display_data) {
    const dd = result.display_data as { type?: string; media_type?: string; data?: string };
    if (dd.type === 'image' && dd.media_type && dd.data) {
      imageResult = { media_type: dd.media_type, data: dd.data };
    }
  }
  if (!imageResult && (name === 'read_image' || name === 'browser_take_screenshot')) {
    imageResult = parseImageResult(resultText);
  }

  // Trivial patch detection: a single-patch call whose diff has ≤3 total
  // changed lines is cheaper to read inline than click-through. We auto-expand
  // it and suppress the (redundant) PatchFileSummary below.
  const patchCount = name === 'patch'
    ? ((input as { patches?: unknown[] }).patches?.length ?? 0)
    : 0;
  const patchLineDelta = name === 'patch' && patchDiff
    ? patchDiff.split('\n').filter(l =>
        (l.startsWith('+') && !l.startsWith('+++')) ||
        (l.startsWith('-') && !l.startsWith('---'))
      ).length
    : 0;
  const isTrivialPatch = name === 'patch' && patchCount === 1 && patchLineDelta <= 3;

  // Determine if output should be auto-expanded.
  // read_file auto-expands regardless of length: the file contents ARE the payload,
  // not supplementary evidence — hiding them defeats the tool's purpose. The
  // 5000-char maxDisplayLen below caps runaway reads.
  const shouldAutoExpand = resultLength > 0 && (
    resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD || name === 'read_file' || isTrivialPatch
  );
  const [outputExpanded, setOutputExpanded] = useState(shouldAutoExpand);

  // For display, truncate very long outputs even when expanded
  const maxDisplayLen = 5000;
  const displayResult = resultText.length > maxDisplayLen 
    ? resultText.slice(0, maxDisplayLen) + `\n... (${resultText.length - maxDisplayLen} more chars)`
    : resultText;

  // Preview for collapsed state: show first 3 lines faded
  const previewLines = resultText.split('\n').slice(0, 3);
  const lineCount = resultText.split('\n').length;
  const hasMoreLines = lineCount > 3;

  const hasOutput = resultContent !== null;
  const isShortOutput = resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;
  const isSubAgentResult = !!(result?.display_data && isSubAgentSummaryData(result.display_data));

  // Get the raw input for copying (not the formatted display)
  const rawInput = name === 'bash' ? String(input['command'] || '') :
                   name === 'think' ? String(input['thoughts'] || '') :
                   name === 'read_file' ? String(input['path'] || '') :
                   name === 'ask_user_question' ? String(((input['questions'] as Array<{ question?: string }> | undefined)?.[0]?.question) || '') :
                   name === 'search' ? String(input['pattern'] || '') :
                   name === 'browser_navigate' ? String(input['url'] || '') :
                   name === 'browser_eval' ? String(input['expression'] || '') :
                   name === 'browser_click' ? String(input['selector'] || '') :
                   name === 'browser_wait_for_selector' ? String(input['selector'] || '') :
                   name === 'browser_type' ? String(input['text'] || '') :
                   JSON.stringify(input, null, 2);

  return (
    <div className="tool-block" data-tool-id={toolId}>
      {/* Tool header with name */}
      <div className="tool-block-header">
        <span className="tool-block-name">{name}</span>
        {hasOutput && (
          <span className={`tool-block-status ${isError ? 'error' : 'success'}`}>
            {isError ? <XIcon /> : <CheckIcon />}
            {durationMs !== undefined && (
              <span className="tool-block-duration">&bull; {formatToolDuration(durationMs)}</span>
            )}
          </span>
        )}
      </div>

      {/* Tool input - always visible */}
      <div className={`tool-block-input ${inputIsMultiline ? 'multiline' : ''}`}>
        {inputDisplay}
        <CopyButton text={rawInput} title="Copy command" />
      </div>

      {/* Tool output - collapsible for long outputs; suppressed when structured summary is shown */}
      {hasOutput && !isSubAgentResult && (
        <div className={`tool-block-output ${isError ? 'error' : ''} ${outputExpanded ? 'expanded' : ''}`}>
          {imageResult ? (
            // Image result: render as image
            <div className="tool-block-image-output">
              <img
                src={`data:${imageResult.media_type};base64,${imageResult.data}`}
                alt="Tool result"
                className="message-image"
              />
            </div>
          ) : isShortOutput ? (
            // Short output: show inline, no collapse
            <div className="tool-block-output-content">
              {displayResult || <span className="tool-empty">(empty)</span>}
              {resultText && <CopyButton text={resultText} title="Copy output" />}
            </div>
          ) : (
            // Long output: collapsible with preview
            <>
              {outputExpanded ? (
                // Expanded: full output with collapse header
                <>
                  <div 
                    className="tool-block-output-header" 
                    onClick={() => setOutputExpanded(false)}
                  >
                    <span className="tool-block-output-chevron"><ChevronDownIcon /></span>
                    <span className="tool-block-output-label">output</span>
                    <span className="tool-block-output-size">{lineCount} lines</span>
                    <CopyButton text={resultText} title="Copy output" />
                  </div>
                  <div className="tool-block-output-content">
                    {displayResult}
                  </div>
                </>
              ) : (
                // Collapsed: show preview lines that expand on click
                <div 
                  className="tool-block-output-preview"
                  onClick={() => setOutputExpanded(true)}
                >
                  <div className="tool-block-preview-lines">
                    {previewLines.map((line, i) => (
                      <div key={i} className="tool-block-preview-line">{line || ' '}</div>
                    ))}
                    {hasMoreLines && (
                      <div className="tool-block-preview-more">+{lineCount - 3} more lines</div>
                    )}
                  </div>
                  <CopyButton text={resultText} title="Copy output" />
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* Patch file summary (REQ-PF-014) */}
      {/* Check display_data.diff first (new format), then fall back to resultText (old format) */}
      {/* Suppressed for trivial patches — the inline diff above already shows everything. */}
      {name === 'patch' && onOpenFile && !isTrivialPatch && (() => {
        const patchDiff = (result?.display_data as { diff?: string })?.diff;
        const diffContent = patchDiff || resultText;
        return diffContent && containsUnifiedDiff(diffContent) ? (
          <PatchFileSummary patchOutput={diffContent} onFileClick={onOpenFile} />
        ) : null;
      })()}

      {/* Sub-agent summary (when subagents complete and update this tool result) */}
      {result?.display_data && isSubAgentSummaryData(result.display_data) && (
        <SubAgentSummary results={result.display_data.results} />
      )}
    </div>
  );
}

// ============================================================================
// Sub-Agent Summary (persistent view after completion)
// ============================================================================

/** Display data format for subagent_summary */
interface SubAgentSummaryData {
  type: 'subagent_summary';
  results: SubAgentResult[];
}

/** Type guard for SubAgentSummaryData */
function isSubAgentSummaryData(data: unknown): data is SubAgentSummaryData {
  return (
    typeof data === 'object' &&
    data !== null &&
    (data as Record<string, unknown>)['type'] === 'subagent_summary' &&
    Array.isArray((data as Record<string, unknown>)['results'])
  );
}

/** Single completed sub-agent row with expandable conversation view */
function SubAgentSummaryRow({ result }: { result: SubAgentResult }) {
  const [conversationExpanded, setConversationExpanded] = useState(false);
  const isError = result.outcome.type === 'failure';
  const resultText = getOutcomeText(result.outcome);

  return (
    <div className={`subagent-summary-row ${isError ? 'error' : ''}`}>
      <div
        className="subagent-summary-header"
        onClick={() => setConversationExpanded(!conversationExpanded)}
      >
        <span className={`subagent-summary-icon ${isError ? 'error' : 'success'}`}>
          {isError ? <XIcon /> : <CheckIcon />}
        </span>
        <span className="subagent-summary-task" title={result.task}>
          {truncate(result.task, 60)}
        </span>
        <span className="subagent-summary-outcome">
          {truncate(resultText, 50)}
        </span>
        <OpenConversationButton agentId={result.agent_id} />
        <span className="subagent-summary-expand">
          {conversationExpanded ? <ChevronUpIcon /> : <ChevronDownIcon />}
        </span>
      </div>
      {conversationExpanded && (
        <div className="subagent-expanded-result">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{resultText}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}

/** Persistent summary of completed subagents (shown in spawn_agents tool result) */
function SubAgentSummary({ results }: { results: SubAgentResult[] }) {
  const successCount = results.filter(r => r.outcome.type === 'success').length;
  const failCount = results.length - successCount;

  return (
    <div className="subagent-summary-block">
      <div className="subagent-summary-title">
        <span className="subagent-summary-stats">
          {successCount > 0 && <span className="success"><CheckIcon /> {successCount}</span>}
          {failCount > 0 && <span className="error"><XIcon /> {failCount}</span>}
        </span>
        <span>completed</span>
      </div>
      <div className="subagent-summary-list">
        {results.map((result) => (
          <SubAgentSummaryRow key={result.agent_id} result={result} />
        ))}
      </div>
    </div>
  );
}

// ============================================================================
// Sub-Agent Status (live progress indicator)
// ============================================================================

/** Truncate text with ellipsis */
function truncate(text: string, maxLen: number): string {
  if (text.length <= maxLen) return text;
  return text.slice(0, maxLen - 1) + '…';
}

/** Get the result text from an outcome */
function getOutcomeText(outcome: SubAgentResult['outcome']): string {
  if (outcome.type === 'success') {
    return outcome.result || 'Completed successfully';
  }
  return outcome.error || 'Failed';
}

const ExternalLinkIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
    <polyline points="15 3 21 3 21 9" />
    <line x1="10" y1="14" x2="21" y2="3" />
  </svg>
);

/**
 * Navigates to a sub-agent's conversation. Sub-agent `agent_id` is the
 * child conversation_id by construction (runtime/executor.rs invariant);
 * the route is keyed by slug, so resolve via cacheDB (populated by the
 * sidebar poll + SSE) with a REST fallback for the rare cache miss.
 * Renders nothing if the conversation can't be resolved (e.g. deleted).
 */
function OpenConversationButton({ agentId }: { agentId: string }) {
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const [missing, setMissing] = useState(false);
  // Synchronous guard against fast double-clicks. `busy` state lags by a
  // render so two clicks fired before React commits would both pass the
  // guard; a ref flips immediately.
  const inFlight = useRef(false);

  const onClick = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      const cached = await cacheDB.getConversation(agentId);
      if (cached?.slug) {
        navigate(`/c/${cached.slug}`);
        return;
      }
      // Cache miss: ask the server. `getConversationSlug` returns null only
      // for 404 (conversation deleted) — that's the one case where we hide
      // the button permanently. Transient failures throw and we leave the
      // button in place so the user can retry.
      const slug = await api.getConversationSlug(agentId);
      if (slug) {
        navigate(`/c/${slug}`);
      } else {
        setMissing(true);
      }
    } catch {
      // Transient error — keep the button enabled so the user can retry.
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }, [agentId, navigate]);

  if (missing) return null;

  return (
    <button
      type="button"
      className="subagent-open-link"
      onClick={onClick}
      title="Open sub-agent conversation"
      aria-label="Open sub-agent conversation"
      disabled={busy}
    >
      <ExternalLinkIcon />
    </button>
  );
}

/** Single completed sub-agent with expandable result */
function CompletedSubAgent({ result }: { result: SubAgentResult }) {
  const [expanded, setExpanded] = useState(false);
  const isError = result.outcome.type === 'failure';
  const resultText = getOutcomeText(result.outcome);
  const hasLongResult = resultText.length > 100;

  return (
    <div className={`subagent-item completed ${isError ? 'error' : ''}`}>
      <div className="subagent-item-header" onClick={() => hasLongResult && setExpanded(!expanded)}>
        <span className="subagent-icon">{isError ? <XIcon /> : <CheckIcon />}</span>
        <span className="subagent-label" title={result.task}>
          {truncate(result.task, 50)}
        </span>
        <OpenConversationButton agentId={result.agent_id} />
        {hasLongResult && (
          <span className="subagent-expand-toggle">
            {expanded ? <ChevronUpIcon /> : <ChevronDownIcon />}
          </span>
        )}
      </div>
      <div className={`subagent-result ${expanded ? 'expanded' : ''}`}>
        {expanded ? resultText : truncate(resultText, 100)}
      </div>
    </div>
  );
}

type AwaitingSubAgentsState = Extract<ConversationState, { type: 'awaiting_sub_agents' }>;

export const SubAgentStatus = memo(SubAgentStatusImpl);

function SubAgentStatusImpl({ stateData }: { stateData: AwaitingSubAgentsState }) {
  const pending: PendingSubAgent[] = stateData.pending;
  const completed: SubAgentResult[] = stateData.completed_results;
  const total = pending.length + completed.length;

  return (
    <div className="subagent-status-block">
      <div className="subagent-header">
        <span className="subagent-title">Sub-agents</span>
        <span className="subagent-count">
          {completed.length}/{total}
        </span>
      </div>
      <div className="subagent-list">
        {completed.map((result) => (
          <CompletedSubAgent key={result.agent_id} result={result} />
        ))}
        {pending.map((agent) => (
          <div key={agent.agent_id} className="subagent-item pending">
            <span className="subagent-icon">
              <span className="spinner"></span>
            </span>
            <span className="subagent-label" title={agent.task}>
              {truncate(agent.task, 50)}
            </span>
            <span className="subagent-status">running...</span>
            <OpenConversationButton agentId={agent.agent_id} />
          </div>
        ))}
      </div>
    </div>
  );
}
