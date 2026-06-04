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

import React, { memo, useState, useMemo, useCallback, useRef, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import { api } from '../api';
import type { Message, ContentBlock, ToolResultContent, ConversationState, PendingSubAgent, SubAgentResult } from '../api';
import type { BashToolInput } from '../generated/sse';
import { cacheDB } from '../cache';
import type { QueuedMessage } from '../hooks';
import { useTheme } from '../hooks/useTheme';
import { useIsDesktop } from '../hooks';
import { useDensity, isSignificantText } from '../hooks/useDensity';
import { useConversationInlineStream } from '../hooks/useConversationInlineStream';
import { useSubAgentViewer } from '../contexts/SubAgentViewerContext';

import { linkifyText } from '../utils/linkify';
import { CopyButton } from './CopyButton';
import { PatchFileSummary, containsUnifiedDiff } from './PatchFileSummary';
import { BrowserProfileResponseView, STRUCTURED_PROFILE_ACTIONS } from './BrowserProfileResponseView';
import { PillStrip, type PillItem } from './PillStrip';
import { deriveToolStripItems, type ToolStripItem } from './agentTurnToolStrip';

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

type DeferredSyntaxHighlighterProps = {
  language: string;
  syntaxStyle: typeof oneDark;
  children: React.ReactNode;
} & Omit<React.ComponentProps<typeof SyntaxHighlighter>, 'language' | 'style' | 'children' | 'PreTag'>;

function scheduleIdleCallback(callback: () => void): () => void {
  const idleWindow = window as typeof window & {
    requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
    cancelIdleCallback?: (handle: number) => void;
  };
  if (idleWindow.requestIdleCallback && idleWindow.cancelIdleCallback) {
    const handle = idleWindow.requestIdleCallback(callback, { timeout: 1500 });
    return () => idleWindow.cancelIdleCallback?.(handle);
  }
  const handle = window.setTimeout(callback, 1500);
  return () => window.clearTimeout(handle);
}

function DeferredSyntaxHighlighter({ language, syntaxStyle, children, ...props }: DeferredSyntaxHighlighterProps) {
  const [highlight, setHighlight] = useState(false);
  const code = String(children).replace(/\n$/, '');

  useEffect(() => scheduleIdleCallback(() => setHighlight(true)), []);

  if (!highlight) {
    return (
      <div className="deferred-code-pre">
        <code className={`deferred-code language-${language}`} {...props}>{code}</code>
      </div>
    );
  }

  return (
    <SyntaxHighlighter
      style={syntaxStyle}
      language={language}
      PreTag="div"
      {...props}
    >
      {code}
    </SyntaxHighlighter>
  );
}

type MarkdownTableProps = React.ComponentPropsWithoutRef<'table'> & { node?: unknown };

function MarkdownTable({ node, children, ...props }: MarkdownTableProps) {
  void node;
  return (
    <div className="markdown-table-scroll">
      <table {...props}>{children}</table>
    </div>
  );
}

// Stable plugin array -- avoids creating a new array reference on every render
const REMARK_PLUGINS = [remarkGfm];
const NO_REMARK_PLUGINS: typeof REMARK_PLUGINS = [];

function usesGfmSyntax(text: string): boolean {
  return /(^|\n)\s*(?:[-*+]|\d+[.)])\s+\[[ xX]\]\s/.test(text)
    || /(^|\n)\s*\|.*\|/.test(text)
    || /(^|\n)\s*[-:| ]{3,}\|[-:| ]{3,}/.test(text)
    || /(?:^|[^~])~[^\s~][^\n~]*[^\s~]~(?:[^~]|$)/.test(text)
    || /~~[^\n]+~~/.test(text)
    || /https?:\/\/\S+/.test(text)
    || /www\.\S+/.test(text)
    || /(^|[^\w.+-])[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/.test(text)
    || /\[\^[^\]]+\]/.test(text);
}

/** Format a tool execution duration for display in the tool block header.
 *  < 1s    -> "743ms" (integer milliseconds)
 *  < 10s   -> "3.2s" (one decimal place)
 *  < 60s   -> "42s"
 *  >= 60s  -> "1m 4s" (seconds part omitted when 0: "2m")
 *
 *  Uses integer millisecond arithmetic so boundary values are exact:
 *  59 999 ms -> "59s", 60 000 ms -> "1m", 119 999 ms -> "1m 59s".
 */
function formatToolDuration(ms: number): string {
  if (ms < 1000) {
    return `${Math.round(ms)}ms`;
  }
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

function isFiniteInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value);
}

function isBashToolInput(input: Record<string, unknown>): input is BashToolInput {
  const op = input['op'];
  if (op !== 'run' && op !== 'peek' && op !== 'wait' && op !== 'kill') return false;
  for (const retiredKey of ['command', 'mode', 'peek', 'wait', 'kill']) {
    if (input[retiredKey] !== undefined) return false;
  }
  if (input['cmd'] !== undefined && typeof input['cmd'] !== 'string') return false;
  if (input['handle'] !== undefined && typeof input['handle'] !== 'string') return false;
  if (input['label'] !== undefined && typeof input['label'] !== 'string') return false;
  if (input['wait_seconds'] !== undefined && (!isFiniteInteger(input['wait_seconds']) || input['wait_seconds'] < 0)) return false;
  if (input['signal'] !== undefined && input['signal'] !== 'TERM' && input['signal'] !== 'KILL') return false;
  if (input['lines'] !== undefined && (!isFiniteInteger(input['lines']) || input['lines'] < 1)) return false;
  if (input['since'] !== undefined && (!isFiniteInteger(input['since']) || input['since'] < 0)) return false;
  return true;
}

function readWindowSuffix(input: Pick<BashToolInput, 'lines' | 'since'>): string {
  if (typeof input.lines === 'number') return ` · last ${input.lines} lines`;
  if (typeof input.since === 'number' && input.since > 0) return ` · since ${input.since}`;
  return '';
}

function formatModernBashInput(input: BashToolInput, displayOverride?: string): { display: string; isMultiline: boolean } {
  switch (input.op) {
    case 'run': {
      const cmd = input.cmd || '';
      if (!cmd) return { display: 'bash run <missing cmd>', isMultiline: false };
      const displayCmd = displayOverride || cmd;
      const waitSuffix = typeof input.wait_seconds === 'number' ? ` · wait ${input.wait_seconds}s` : '';
      return { display: `$ ${displayCmd}${waitSuffix}${readWindowSuffix(input)}`, isMultiline: cmd.includes('\n') };
    }
    case 'peek': {
      const handle = input.handle || '<missing handle>';
      return { display: `peek ${handle}${readWindowSuffix(input)}`, isMultiline: false };
    }
    case 'wait': {
      const handle = input.handle || '<missing handle>';
      const waitSuffix = typeof input.wait_seconds === 'number' ? ` (up to ${input.wait_seconds}s)` : '';
      return { display: `wait ${handle}${waitSuffix}${readWindowSuffix(input)}`, isMultiline: false };
    }
    case 'kill': {
      const handle = input.handle || '<missing handle>';
      const signal = input.signal || 'TERM';
      return { display: `kill ${handle} (${signal})`, isMultiline: false };
    }
  }
}

function bashInputCopyText(input: Record<string, unknown>): string {
  if (isBashToolInput(input)) {
    if (input.op === 'run') return input.cmd || JSON.stringify(input);
    return JSON.stringify(input);
  }
  if (input['op'] === undefined) {
    const legacyText = input['command'] || input['cmd'] || input['peek'] || input['wait'] || input['kill'];
    if (legacyText) return String(legacyText);
  }
  return JSON.stringify(input, null, 2);
}

function formatToolInput(name: string, input: Record<string, unknown>, displayOverride?: string): { display: string; isMultiline: boolean } {
  switch (name) {
    case 'bash': {
      if (isBashToolInput(input)) {
        return formatModernBashInput(input, displayOverride);
      }
      const legacyCommand = input['op'] === undefined ? String(input['command'] || input['cmd'] || '') : '';
      if (legacyCommand) {
        const displayCmd = displayOverride || legacyCommand;
        return { display: `$ ${displayCmd}`, isMultiline: legacyCommand.includes('\n') };
      }
      const legacyJson = JSON.stringify(input);
      return { display: `bash ${legacyJson}`, isMultiline: false };
    }
    case 'tmux': {
      const args = (input['args'] as unknown[] | undefined) ?? [];
      const argList = args.map((a) => String(a)).join(' ');
      return { display: `tmux ${argList}`, isMultiline: false };
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
    case 'browser_profile': {
      const action = String(input['action'] || '');
      switch (action) {
        case 'run_scenario': {
          const runs = input['runs'] ?? 1;
          const warmup = input['warmup'] ?? 1;
          const steps = Array.isArray(input['steps']) ? (input['steps'] as unknown[]).length : 0;
          const tr = input['throttle_rate'];
          const thr = tr !== undefined && tr !== null ? `, throttle ${String(tr)}x` : '';
          const reset = input['reset'];
          const resetStr =
            reset === 'none'
              ? ', reset:none'
              : reset && typeof reset === 'object'
                ? `, reset:${String((reset as Record<string, unknown>)['kind'] ?? '?')}`
                : '';
          const gcStr = input['gc_per_run'] === false ? ', gc:off' : '';
          return `profile: scenario (${steps} steps × ${String(runs)} runs, ${String(warmup)} warmup${thr}${resetStr}${gcStr})`;
        }
        case 'throttle':
          return `profile: throttle ${String(input['rate'] ?? '')}x`;
        case 'trace_start': {
          const cats = input['categories'] ? ` [${String(input['categories'])}]` : '';
          return `profile: trace_start${cats}`;
        }
        case 'heap_snapshot':
          return input['baseline'] ? 'profile: heap_snapshot (diff)' : 'profile: heap_snapshot';
        default:
          return action ? `profile: ${action}` : 'profile';
      }
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
// the entry it receives is either `pending` or `steering_queued` (task 02676).
function QueuedUserMessageImpl({
  message,
  onCancelSteering,
}: {
  message: QueuedMessage;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
}) {
  const isSteeringQueued = message.status === 'steering_queued';
  return (
    <div className={`message user${isSteeringQueued ? ' steering-queued' : ''}`}>
      <div className="message-header">
        <span className="message-sender">You</span>
        {isSteeringQueued ? (
          <span className="message-status queued" title="Queued — will send when conversation is free">
            <span className="queued-label">⏳ Queued</span>
            {onCancelSteering && (
              <button
                className="cancel-steering-btn"
                title="Cancel queued message"
                onClick={() => onCancelSteering(message.localId)}
              >
                ×
              </button>
            )}
          </span>
        ) : (
          <span className="message-status sending" title="Sending...">
            <span className="sending-spinner">⏳</span>
          </span>
        )}
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
// Compact-density helpers
// ============================================================================

/** First non-empty line of a text block, collapsed to single-line whitespace
 *  and ellipsized — the faded one-liner shown for insignificant prose in
 *  compact mode. */
function firstLineSummary(text: string, maxLen = 140): string {
  const firstLine = text.split('\n').find((l) => l.trim()) ?? text;
  const flat = firstLine.replace(/\s+/g, ' ').trim();
  return flat.length > maxLen ? `${flat.slice(0, maxLen - 1)}…` : flat;
}

/**
 * An assistant text block that, in compact mode, is below the significance
 * threshold. Renders as a faded clickable one-liner that expands to the full
 * markdown on click — never destructive, the full text is always one click
 * away (and the title attr carries the first line for hover).
 */
const CollapsibleText = memo(CollapsibleTextImpl);

function CollapsibleTextImpl({
  text,
  remarkPlugins,
  components,
}: {
  text: string;
  remarkPlugins: typeof REMARK_PLUGINS;
  components: React.ComponentProps<typeof ReactMarkdown>['components'];
}) {
  const [expanded, setExpanded] = useState(false);

  if (expanded) {
    return (
      <div className="agent-text-block">
        <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
          {text}
        </ReactMarkdown>
      </div>
    );
  }

  const summary = firstLineSummary(text);
  return (
    <div
      className="agent-text-collapsed"
      role="button"
      tabIndex={0}
      title={summary}
      onClick={() => setExpanded(true)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          setExpanded(true);
        }
      }}
    >
      {summary}
    </div>
  );
}

/**
 * The inline mini pill-strip a compact-mode agent turn shows in place of its
 * tool blocks. Built purely from the turn's own tool_use blocks + paired
 * results (via `deriveToolStripItems`), never from phase/breadcrumb state.
 * Clicking any pill calls `onExpand(toolId)` so the parent can reveal the full
 * tool detail and scroll the clicked tool into view.
 */
const CompactToolStrip = memo(CompactToolStripImpl);

function CompactToolStripImpl({
  items,
  onExpand,
}: {
  items: ToolStripItem[];
  onExpand: (toolId: string) => void;
}) {
  const pills: PillItem[] = useMemo(
    () =>
      items.map((item, i) => {
        const variant = item.isSubAgent ? 'subagents' : 'tool';
        const classNames = [variant, item.isError ? 'error' : '', !item.hasResult ? 'pending' : '']
          .filter(Boolean)
          .join(' ');
        return {
          key: item.toolId || `${item.name}-${i}`,
          label: item.name,
          className: classNames,
          ariaLabel: `${item.name}${item.isError ? ' (error)' : ''} — expand tool detail`,
          onClick: () => onExpand(item.toolId),
        };
      }),
    [items, onExpand],
  );

  return (
    <div className="compact-tool-strip">
      <PillStrip
        items={pills}
        pillClassName="compact-tool-pill breadcrumb-item"
        arrowClassName="breadcrumb-arrow"
      />
    </div>
  );
}

// ============================================================================
// Agent Message Components
// ============================================================================

interface AgentMessageProps {
  message: Message;
  toolResults: ReadonlyMap<string, Message>;
  onOpenFile?: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  filePathRootDir?: string | undefined;
  /**
   * When false, suppresses the "Phoenix HH:MM" header row. Used by the list
   * to collapse repeated headers across a run of consecutive agent messages
   * within the same turn. Defaults to true so callers that don't set it keep
   * the original behavior.
   */
  isFirstInTurn?: boolean;
}

export const AgentMessage = memo(AgentMessageImpl);

function AgentMessageImpl({ message, toolResults, onOpenFile, filePathRootDir, isFirstInTurn = true }: AgentMessageProps) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const timestamp = message.created_at;
  const { theme } = useTheme();
  const { density } = useDensity();
  const compact = density === 'compact';
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;

  // In compact mode, a turn's tool_use blocks collapse into a single inline
  // pill strip rendered in place of the tool blocks; clicking a pill expands
  // the full per-tool detail and scrolls the clicked tool into view.
  // `think` blocks are never part of the strip — they always render as their
  // own self-collapsing aside, in both densities.
  const [toolsExpanded, setToolsExpanded] = useState(false);
  // The tool the user clicked to expand; scrolled into view once the detail
  // mounts. Cleared after the scroll so re-expansion is idempotent.
  const pendingScrollToolIdRef = useRef<string | null>(null);

  // Derived purely from this turn's own content blocks + paired results — the
  // turn is the single source of truth for what it did (never phase state).
  const toolStripItems = useMemo(
    () => deriveToolStripItems(message, toolResults),
    [message, toolResults],
  );

  const handleExpandTools = useCallback((toolId: string) => {
    pendingScrollToolIdRef.current = toolId || null;
    setToolsExpanded(true);
  }, []);

  // After the full tool detail mounts, scroll the clicked tool into view.
  useEffect(() => {
    if (!toolsExpanded) return;
    const toolId = pendingScrollToolIdRef.current;
    pendingScrollToolIdRef.current = null;
    if (!toolId) return;
    const el = document.querySelector(`[data-tool-id="${toolId}"]`);
    el?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }, [toolsExpanded]);

  // Compact mode collapses tools only when there is at least one strip item;
  // a turn of pure prose / think asides has nothing to collapse.
  const collapseTools = compact && !toolsExpanded && toolStripItems.length > 0;

  const filePathCopyContext = useMemo(
    () => (filePathRootDir ? { rootDir: filePathRootDir } : undefined),
    [filePathRootDir],
  );


  // Stable markdown component map — only recreated when onOpenFile identity changes.
  // Keeps ReactMarkdown from remounting SyntaxHighlighter on every parent re-render.
  const markdownComponents = useMemo(() => ({
    // Custom code block rendering with syntax highlighting
    // Inline code with file paths becomes clickable
    code: ({ inline, className, children, node, ...props }: { inline?: boolean | undefined; className?: string | undefined; children?: React.ReactNode; node?: unknown }) => {
      void node;
      const match = /language-(\w+)/.exec(className || '');
      if (!inline && match?.[1]) {
        return (
          <DeferredSyntaxHighlighter
            syntaxStyle={syntaxStyle}
            language={match[1]}
            {...props}
          >
            {children}
          </DeferredSyntaxHighlighter>
        );
      }
      // For inline code, check if it looks like a file path and make it clickable
      const text = String(children);
      const fileClickHandler = onOpenFile
        ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
        : undefined;
      const linkified = linkifyText(text, fileClickHandler, filePathCopyContext);
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
            return linkifyText(child, fileClickHandler, filePathCopyContext);
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
            return linkifyText(child, fileClickHandler, filePathCopyContext);
          }
          return child;
        });
      };
      return <li>{processChildren(children)}</li>;
    },
    table: MarkdownTable,
  }), [onOpenFile, filePathCopyContext, syntaxStyle]);

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
          {/* REQ-LRV-006: post-hoc retry badge. The runtime stamps
              `display_data.retry_count` on the persisted assistant
              message iff the turn retried (max(0, final_attempt - 1)).
              Zero is encoded as "field absent" on the JSON, so the
              `> 0` check doubles as a presence check. The badge is
              the long-lived record of "this answer took N tries" once
              the live retry suffix on the StateBar has cleared. */}
          {(() => {
            const dd = message.display_data as Record<string, unknown> | undefined;
            const retryCount = typeof dd?.['retry_count'] === 'number' ? (dd['retry_count'] as number) : 0;
            if (retryCount > 0) {
              return (
                <span
                  className="message-retry-badge"
                  title={`This response succeeded after ${retryCount} retry attempt${retryCount === 1 ? '' : 's'}.`}
                >
                  retried {retryCount}x
                </span>
              );
            }
            return null;
          })()}
        </div>
      )}
      <div className="message-content">
        {(() => {
          // In compact mode, all of a turn's tool blocks collapse into one
          // inline strip painted at the position of the first non-think tool
          // block. We render it once and suppress the individual full tool
          // blocks below; `think` asides still render inline (they're
          // reasoning, not actions). A sentinel tracks whether the strip has
          // already been emitted so it lands in document order.
          let stripEmitted = false;
          return blocks.map((block, i) => {
            if (block.type === 'text') {
              // Skip empty text blocks - they produce empty bubbles
              if (!block.text || block.text.trim() === '') {
                return null;
              }
              const remarkPlugins = usesGfmSyntax(block.text) ? REMARK_PLUGINS : NO_REMARK_PLUGINS;
              // Compact: short prose folds to a faded expandable one-liner.
              // Substantial prose (>= threshold) always renders full.
              if (compact && !isSignificantText(block.text)) {
                return (
                  <CollapsibleText
                    key={i}
                    text={block.text}
                    remarkPlugins={remarkPlugins}
                    components={markdownComponents}
                  />
                );
              }
              return (
                <div key={i} className="agent-text-block">
                  <ReactMarkdown remarkPlugins={remarkPlugins} components={markdownComponents}>
                    {block.text}
                  </ReactMarkdown>
                </div>
              );
            } else if (block.type === 'tool_use') {
              // `think` renders as a subtle inline aside, not the full tool-block
              // shell — it's model reasoning, not an action. Collapsed by default,
              // identical in both densities.
              if (block.name === 'think') {
                return <ThinkAside key={block.id || i} block={block} />;
              }
              // Compact + not yet expanded: paint the collapsed strip once, in
              // place of the first tool block; suppress the rest.
              if (collapseTools) {
                if (stripEmitted) return null;
                stripEmitted = true;
                return (
                  <CompactToolStrip
                    key="compact-tool-strip"
                    items={toolStripItems}
                    onExpand={handleExpandTools}
                  />
                );
              }
              // REQ-WPV-002: read the per-tool start time from the
              // parent assistant message's `display_data.tool_starts`
              // (typed `{ [tool_use_id]: unix_ms }` on the Rust side).
              // When present, the widget renders a live elapsed counter
              // that survives reconnect / reload / multi-tab.
              const toolStartsMap = (message.display_data as Record<string, unknown> | undefined)?.[
                'tool_starts'
              ] as Record<string, number> | undefined;
              const toolStartedAtMs =
                block.id && toolStartsMap && typeof toolStartsMap[block.id] === 'number'
                  ? (toolStartsMap[block.id] as number)
                  : undefined;
              return (
                <ToolUseBlock
                  key={block.id || i}
                  block={block}
                  result={toolResults.get(block.id || '')}
                  onOpenFile={onOpenFile}
                  toolStartedAtMs={toolStartedAtMs}
                />
              );
            }
            return null;
          });
        })()}
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
  /** Server-clock unix ms when the runtime began dispatching this
   *  tool — sourced from the parent assistant message's
   *  `display_data.tool_starts[block.id]` (REQ-WPV-002). When present
   *  and no `result` has landed yet, the tool widget renders a live
   *  elapsed counter that survives reconnect / reload / multi-tab. */
  toolStartedAtMs?: number | undefined;
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

// Permissive JSON parse used for typed tool results (bash, tmux). Returns
// the parsed object or null when the content is plain text. Anything that
// isn't an object (string / number / bool) returns null — we only care
// about the structured response shape.
function tryParseJson(text: string): Record<string, unknown> | null {
  if (!text) return null;
  try {
    const parsed = JSON.parse(text);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // Not JSON — typically a plain-text legacy bash output.
  }
  return null;
}

// REQ-BASH-002 / REQ-BASH-003 / REQ-BASH-006: render the typed bash tool
// response. Renders a status pill, optional kill-pending badge, the line
// tail, and (when present) the agent-supplied `label` so concurrent
// handles are distinguishable at a glance.
function BashResponseView({ response }: { response: Record<string, unknown> }) {
  // Error envelope branch (REQ-BASH-008): `error` field present.
  if (typeof response['error'] === 'string') {
    return <BashErrorView response={response} />;
  }
  const status = String(response['status'] ?? '');
  const handle = typeof response['handle'] === 'string' ? response['handle'] : null;
  const label = typeof response['label'] === 'string' ? response['label'] : null;
  const finalCause = typeof response['final_cause'] === 'string' ? response['final_cause'] : null;
  const exitCode = response['exit_code'];
  const signalNumber = response['signal_number'];
  const signalSent = typeof response['signal_sent'] === 'string' ? response['signal_sent'] : null;
  const killSignalSent =
    typeof response['kill_signal_sent'] === 'string' ? response['kill_signal_sent'] : null;
  const waitedMs = typeof response['waited_ms'] === 'number' ? response['waited_ms'] : null;
  const durationMs = typeof response['duration_ms'] === 'number' ? response['duration_ms'] : null;
  const truncatedBefore = response['truncated_before'] === true;
  const lines = Array.isArray(response['lines'])
    ? (response['lines'] as Array<{ offset?: number; bytes?: string }>)
    : [];

  const isExited = status === 'exited';
  const isKilled = status === 'killed';
  const isTombstone = status === 'tombstoned';

  const text = lines.map((l) => l.bytes ?? '').join('\n');

  return (
    <div className="bash-response">
      <div className="bash-response-header">
        <span className={`bash-status bash-status-${status.replace(/_/g, '-')}`}>
          {status === 'running'
            ? 'running'
            : status === 'still_running'
              ? 'still running'
              : status === 'kill_pending_kernel'
                ? 'kill pending (kernel)'
                : status === 'tombstoned'
                  ? `tombstoned${finalCause ? ` · ${finalCause}` : ''}`
                  : status === 'exited'
                    ? 'exited'
                    : status === 'killed'
                      ? 'killed'
                      : status}
        </span>
        {handle && <span className="bash-handle">{handle}</span>}
        {label && <span className="bash-label" title="agent-supplied handle label">{label}</span>}
        {(isExited || isTombstone) && exitCode !== undefined && exitCode !== null && (
          <span className="bash-exit-code">exit code {String(exitCode)}</span>
        )}
        {(isKilled || isTombstone) && typeof signalNumber === 'number' && (
          <span className="bash-signal-number">signal {String(signalNumber)}</span>
        )}
        {killSignalSent && (
          <span className="bash-kill-signal">kill: {killSignalSent}</span>
        )}
        {signalSent && signalSent !== killSignalSent && (
          <span className="bash-signal-sent">signal_sent: {signalSent}</span>
        )}
        {waitedMs !== null && (
          <span className="bash-duration">waited {Math.round(waitedMs)} ms</span>
        )}
        {durationMs !== null && (
          <span className="bash-duration">duration {Math.round(durationMs)} ms</span>
        )}
      </div>
      {truncatedBefore && (
        <div className="bash-truncated-notice">[output truncated before this view]</div>
      )}
      {text && <pre className="bash-lines">{text}</pre>}
    </div>
  );
}

function BashErrorView({ response }: { response: Record<string, unknown> }) {
  const error = String(response['error'] ?? '');
  const message = typeof response['error_message'] === 'string' ? response['error_message'] : '';
  const hint = typeof response['hint'] === 'string' ? response['hint'] : null;
  return (
    <div className="bash-response bash-response-error">
      <div className="bash-response-header">
        <span className="bash-status bash-status-error">error: {error}</span>
      </div>
      {message && <div className="bash-error-message">{message}</div>}
      {hint && <div className="bash-error-hint">{hint}</div>}
    </div>
  );
}

// REQ-TMUX-012: render the typed tmux tool response. stdout / stderr are
// kept separate (the bash tool's combined ring is different).
function TmuxResponseView({ response }: { response: Record<string, unknown> }) {
  if (typeof response['error'] === 'string') {
    const error = String(response['error']);
    const message = typeof response['message'] === 'string' ? response['message'] : '';
    return (
      <div className="tmux-response tmux-response-error">
        <span className="tmux-status tmux-status-error">error: {error}</span>
        {message && <div className="tmux-error-message">{message}</div>}
      </div>
    );
  }
  const status = String(response['status'] ?? '');
  const exitCode = response['exit_code'];
  const durationMs = typeof response['duration_ms'] === 'number' ? response['duration_ms'] : null;
  const stdout = typeof response['stdout'] === 'string' ? response['stdout'] : '';
  const stderr = typeof response['stderr'] === 'string' ? response['stderr'] : '';
  const truncated = response['truncated'] === true;
  return (
    <div className="tmux-response">
      <div className="tmux-response-header">
        <span className={`tmux-status tmux-status-${status.replace(/_/g, '-')}`}>{status}</span>
        {exitCode !== undefined && exitCode !== null && (
          <span className="tmux-exit-code">exit code {String(exitCode)}</span>
        )}
        {durationMs !== null && (
          <span className="tmux-duration">{Math.round(durationMs)} ms</span>
        )}
      </div>
      {stdout && (
        <div className="tmux-stream">
          <div className="tmux-stream-label">stdout</div>
          <pre className="tmux-stream-content">{stdout}</pre>
        </div>
      )}
      {stderr && (
        <div className="tmux-stream tmux-stream-stderr">
          <div className="tmux-stream-label">stderr</div>
          <pre className="tmux-stream-content">{stderr}</pre>
        </div>
      )}
      {truncated && <div className="tmux-truncated-notice">[output truncated]</div>}
    </div>
  );
}

// Console logs come as a JSON array `[{level, text}, ...]` newest-first, or — when
// the encoded form would blow past the 4KB result cap — a `"Logs written to /tmp/..."`
// pointer string. Render by level so error/warning entries don't disappear in a wall
// of debug logs.
type ConsoleLogEntry = { level: string; text: string };

const CONSOLE_LEVEL_ORDER = ['error', 'warning', 'info', 'log', 'debug'] as const;

function parseConsoleLogs(text: string): ConsoleLogEntry[] | null {
  try {
    const parsed = JSON.parse(text);
    if (!Array.isArray(parsed)) return null;
    const entries: ConsoleLogEntry[] = [];
    for (const e of parsed) {
      if (e && typeof e === 'object' && typeof (e as { level?: unknown }).level === 'string' && typeof (e as { text?: unknown }).text === 'string') {
        entries.push({ level: (e as ConsoleLogEntry).level, text: (e as ConsoleLogEntry).text });
      }
    }
    return entries;
  } catch {
    return null;
  }
}

export function BrowserConsoleLogsView({ rawText }: { rawText: string }) {
  const trimmed = rawText.trim();
  // File escape-hatch path (REQ-BT-015): output exceeds 4KB, full logs dumped to disk.
  if (trimmed.startsWith('Logs written to ')) {
    return (
      <div className="console-logs-response">
        <div className="console-logs-pointer">{trimmed}</div>
      </div>
    );
  }

  const entries = parseConsoleLogs(rawText);
  if (entries === null) {
    return <pre className="console-logs-fallback">{rawText}</pre>;
  }
  if (entries.length === 0) {
    return <div className="console-logs-response"><div className="console-logs-empty">(no console entries)</div></div>;
  }

  const counts: Record<string, number> = {};
  for (const e of entries) counts[e.level] = (counts[e.level] ?? 0) + 1;

  return (
    <div className="console-logs-response">
      <div className="console-logs-header">
        <span className="console-logs-count">
          {entries.length} entr{entries.length === 1 ? 'y' : 'ies'}
        </span>
        {CONSOLE_LEVEL_ORDER.map((lvl) =>
          counts[lvl] ? (
            <span key={lvl} className={`console-logs-tally console-level-${lvl}`}>
              {counts[lvl]} {lvl}
            </span>
          ) : null
        )}
      </div>
      <div className="console-logs-list">
        {entries.map((entry, i) => (
          <div key={i} className={`console-log-entry console-level-${entry.level}`}>
            <span className="console-log-level">{entry.level}</span>
            <div className="console-log-text">{entry.text}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

// Search tool output is plain text shaped as `relative/path:NN: content` lines,
// optionally followed by bracketed notes like `[Results limited to 50 ...]` or
// `[Walk truncated ...]`. Group hits by file so a multi-hit file shows once with
// its line numbers underneath.
type SearchHit = { path: string; lineNumber: number; content: string };

// eslint-disable-next-line react-refresh/only-export-components
export function parseSearchOutput(text: string): {
  hits: SearchHit[];
  notes: string[];
  noMatches: boolean;
} {
  const notes: string[] = [];
  const hits: SearchHit[] = [];

  if (text.trim() === 'No matches found.') {
    return { hits, notes, noMatches: true };
  }

  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    if (line.startsWith('[') && line.trimEnd().endsWith(']')) {
      notes.push(line.trim().slice(1, -1));
      continue;
    }
    // Non-greedy path, then :digits:, then optional space, then content.
    // Path can contain colons in unusual cases; backtracking will find the
    // rightmost path/digits boundary that satisfies the digit run.
    const m = /^(.+?):(\d+):\s?(.*)$/.exec(line);
    if (m && m[1] !== undefined && m[2] !== undefined) {
      hits.push({ path: m[1], lineNumber: parseInt(m[2], 10), content: m[3] ?? '' });
    } else {
      notes.push(line);
    }
  }
  return { hits, notes, noMatches: false };
}

export function SearchResultsView({
  rawText,
  onOpenFile,
}: {
  rawText: string;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
}) {
  const { hits, notes, noMatches } = useMemo(() => parseSearchOutput(rawText), [rawText]);

  if (noMatches) {
    return (
      <div className="search-results">
        <div className="search-results-empty">No matches found.</div>
      </div>
    );
  }

  if (hits.length === 0 && notes.length === 0) {
    return <pre className="search-results-fallback">{rawText}</pre>;
  }

  const groups: Array<{ path: string; hits: SearchHit[] }> = [];
  const seen = new Map<string, number>();
  for (const hit of hits) {
    const idx = seen.get(hit.path);
    if (idx === undefined) {
      seen.set(hit.path, groups.length);
      groups.push({ path: hit.path, hits: [hit] });
    } else {
      groups[idx]!.hits.push(hit);
    }
  }

  return (
    <div className="search-results">
      {hits.length > 0 && (
        <div className="search-results-header">
          <span className="search-results-count">
            {hits.length} match{hits.length === 1 ? '' : 'es'} in {groups.length} file
            {groups.length === 1 ? '' : 's'}
          </span>
        </div>
      )}
      <div className="search-results-list">
        {groups.map((group) => (
          <div key={group.path} className="search-results-file">
            {onOpenFile ? (
              <button
                type="button"
                className="search-results-filepath"
                onClick={() =>
                  onOpenFile(
                    group.path,
                    new Set([group.hits[0]!.lineNumber]),
                    group.hits[0]!.lineNumber
                  )
                }
                title="Open file"
              >
                {group.path}
                <span className="search-results-filehit-count">
                  {group.hits.length} hit{group.hits.length === 1 ? '' : 's'}
                </span>
              </button>
            ) : (
              <span className="search-results-filepath search-results-filepath-static">
                {group.path}
                <span className="search-results-filehit-count">
                  {group.hits.length} hit{group.hits.length === 1 ? '' : 's'}
                </span>
              </span>
            )}
            <div className="search-results-hits">
              {group.hits.map((hit, i) =>
                onOpenFile ? (
                  <button
                    key={i}
                    type="button"
                    className="search-result-line search-result-line-clickable"
                    onClick={() =>
                      onOpenFile(group.path, new Set([hit.lineNumber]), hit.lineNumber)
                    }
                  >
                    <span className="search-result-lineno">{hit.lineNumber}</span>
                    <span className="search-result-content">{hit.content || ' '}</span>
                  </button>
                ) : (
                  <div key={i} className="search-result-line">
                    <span className="search-result-lineno">{hit.lineNumber}</span>
                    <span className="search-result-content">{hit.content || ' '}</span>
                  </div>
                )
              )}
            </div>
          </div>
        ))}
      </div>
      {notes.length > 0 && (
        <div className="search-results-notes">
          {notes.map((n, i) => (
            <div key={i} className="search-results-note">{n}</div>
          ))}
        </div>
      )}
    </div>
  );
}

// keyword_search returns LLM-filtered text shaped as `path: explanation` per line,
// or — when the filter LLM is unavailable — raw ripgrep output (with line numbers
// and context separators). Detect which and render accordingly.
type KeywordHit = { path: string; explanation: string };

// eslint-disable-next-line react-refresh/only-export-components
export function parseKeywordSearchOutput(text: string): {
  hits: KeywordHit[];
  rawFallback: boolean;
  empty: boolean;
} {
  const trimmed = text.trim();
  if (
    trimmed === '' ||
    trimmed === 'No matches found for the given search terms.' ||
    trimmed.startsWith('No relevant files found')
  ) {
    return { hits: [], rawFallback: false, empty: true };
  }

  const lines = text.split('\n').filter((l) => l.trim());
  // Raw ripgrep -C output has `path:NN:` or `path-NN-` per line plus `--` separators.
  // If a meaningful fraction of lines look like that, treat as fallback.
  const ripgrepShaped = lines.filter((l) => /^[^\s].*?[-:]\d+[-:]/.test(l) || l === '--').length;
  if (lines.length >= 4 && ripgrepShaped / lines.length > 0.25) {
    return { hits: [], rawFallback: true, empty: false };
  }

  const hits: KeywordHit[] = [];
  for (const line of lines) {
    // path is everything up to the first `: ` (with a trailing space), and must
    // not itself contain a colon — the LLM-filter prompt's output uses absolute
    // POSIX paths with `: ` as the separator before the explanation.
    const m = /^([^:\s][^:]*?):\s+(.+)$/.exec(line);
    if (m && m[1] !== undefined && m[2] !== undefined) {
      hits.push({ path: m[1].trim(), explanation: m[2].trim() });
    }
  }

  // If very few lines parsed cleanly, the output likely isn't the LLM-filtered
  // shape — bail to plain rendering rather than show a tiny misleading list.
  if (hits.length === 0 || hits.length * 3 < lines.length) {
    return { hits: [], rawFallback: true, empty: false };
  }
  return { hits, rawFallback: false, empty: false };
}

export function KeywordSearchView({
  rawText,
  onOpenFile,
}: {
  rawText: string;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
}) {
  const parsed = useMemo(() => parseKeywordSearchOutput(rawText), [rawText]);

  if (parsed.empty) {
    return (
      <div className="keyword-search-results">
        <div className="search-results-empty">No relevant files found.</div>
      </div>
    );
  }

  if (parsed.rawFallback) {
    return (
      <div className="keyword-search-results keyword-search-raw">
        <div className="keyword-search-fallback-note">
          Raw ripgrep results — LLM filter unavailable
        </div>
        <pre className="keyword-search-raw-text">{rawText}</pre>
      </div>
    );
  }

  return (
    <div className="keyword-search-results">
      <div className="search-results-header">
        <span className="search-results-count">
          {parsed.hits.length} relevant file{parsed.hits.length === 1 ? '' : 's'}
        </span>
      </div>
      <div className="keyword-search-list">
        {parsed.hits.map((hit, i) => (
          <div key={i} className="keyword-search-hit">
            {onOpenFile ? (
              <button
                type="button"
                className="keyword-search-filepath"
                onClick={() => onOpenFile(hit.path, new Set(), 0)}
              >
                {hit.path}
              </button>
            ) : (
              <span className="keyword-search-filepath keyword-search-filepath-static">
                {hit.path}
              </span>
            )}
            <div className="keyword-search-explanation">{hit.explanation}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

export const ToolUseBlock = memo(ToolUseBlockImpl);

function ToolUseBlockImpl({ block, result, onOpenFile, toolStartedAtMs }: ToolUseBlockProps) {
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

  // REQ-WPV-002: live elapsed counter while the tool is in flight
  // (block exists, no result yet). Source is the server-authoritative
  // `tool_starts[block.id]` stamped by `dispatch_tool_execution`, so
  // the counter survives reconnect / reload / multi-tab. Cleared the
  // instant the result lands (the static `durationMs` from the tool
  // result takes over).
  const [inflightElapsedSeconds, setInflightElapsedSeconds] = useState(0);
  useEffect(() => {
    if (result != null || toolStartedAtMs == null) {
      setInflightElapsedSeconds(0);
      return;
    }
    const compute = () =>
      setInflightElapsedSeconds(Math.max(0, Math.floor((Date.now() - toolStartedAtMs) / 1000)));
    compute();
    const interval = window.setInterval(compute, 1000);
    return () => window.clearInterval(interval);
  }, [result, toolStartedAtMs]);

  const rawResultText = resultContent?.content || resultContent?.result || resultContent?.error || '';
  const isError = resultContent?.is_error || !!resultContent?.error;

  // For bash/tmux, the tool result is a structured JSON envelope (REQ-BASH-002 /
  // REQ-TMUX-012). Decode it once so the renderer below can branch on
  // status / running state / label rather than show the raw JSON.
  const bashResponse = name === 'bash' ? tryParseJson(rawResultText) : null;
  const tmuxResponse = name === 'tmux' ? tryParseJson(rawResultText) : null;

  // For patch tool, use the diff from display_data instead of the generic success message
  const patchDiff = name === 'patch' ? (result?.display_data as { diff?: string })?.diff : undefined;
  const resultText = patchDiff || rawResultText;
  const resultLength = resultText.length;
  
  // Check if this is an image result.
  // 1. Typed `images` channel (read_image — single source of truth, no
  //    longer duplicated into display_data).
  // 2. display_data (browser_take_screenshot; also legacy read_image rows
  //    persisted before the payload was removed from display_data).
  // 3. parseImageResult fallback for the oldest legacy rows.
  let imageResult: { media_type: string; data: string } | null = null;
  const typedImage = resultContent?.images?.[0];
  if (typedImage?.data && typedImage.media_type) {
    imageResult = { media_type: typedImage.media_type, data: typedImage.data };
  }
  if (!imageResult && result?.display_data) {
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
  const rawInput = name === 'bash' ? bashInputCopyText(input as Record<string, unknown>) :
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

  const bashCopyTitle = name === 'bash' && isBashToolInput(input as Record<string, unknown>) && (input as BashToolInput).op !== 'run'
    ? 'Copy operation'
    : 'Copy command';

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
        {/* REQ-WPV-002: live elapsed counter while the tool is in
            flight. Hidden once the result lands (the static
            duration above takes over). Server-clock sourced — the
            counter ticks correctly across reconnect / reload. */}
        {result == null && toolStartedAtMs != null && (
          <span
            className="tool-block-elapsed"
            title={`Started ${new Date(toolStartedAtMs).toLocaleTimeString()}`}
          >
            &bull; {inflightElapsedSeconds}s
          </span>
        )}
      </div>

      {/* Tool input - always visible */}
      <div className={`tool-block-input ${inputIsMultiline ? 'multiline' : ''}`}>
        {inputDisplay}
        <CopyButton text={rawInput} title={bashCopyTitle} />
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
          ) : bashResponse ? (
            <BashResponseView response={bashResponse} />
          ) : tmuxResponse ? (
            <TmuxResponseView response={tmuxResponse} />
          ) : name === 'browser_profile' &&
              STRUCTURED_PROFILE_ACTIONS.has(
                String((input as Record<string, unknown>)?.['action'] ?? ''),
              ) ? (
            <BrowserProfileResponseView
              action={String((input as Record<string, unknown>)?.['action'] ?? '')}
              displayData={result?.display_data as Record<string, unknown> | undefined}
              fallbackText={resultText}
              isError={isError}
            />
          ) : name === 'browser_recent_console_logs' && !isError ? (
            <BrowserConsoleLogsView rawText={resultText} />
          ) : name === 'search' && !isError ? (
            <SearchResultsView rawText={resultText} onOpenFile={onOpenFile} />
          ) : name === 'keyword_search' && !isError ? (
            <KeywordSearchView rawText={resultText} onOpenFile={onOpenFile} />
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

type SubAgentStatusKind = 'running' | 'success' | 'failure' | 'timed_out';

function statusKindFromOutcome(outcome: SubAgentResult['outcome'] | null): SubAgentStatusKind {
  if (!outcome) return 'running';
  return outcome.type;
}

function getStatusLabel(status: SubAgentStatusKind): string {
  switch (status) {
    case 'running': return 'running…';
    case 'success': return 'success';
    case 'failure': return 'failed';
    case 'timed_out': return 'timed out';
    default: status satisfies never; return '';
  }
}

function getOutcomeText(outcome: SubAgentResult['outcome']): string {
  switch (outcome.type) {
    case 'success': return outcome.result || 'Completed successfully';
    case 'failure': return outcome.error || 'Failed';
    case 'timed_out': return 'Timed out: sub-agent exceeded its time limit';
    default: outcome satisfies never; return '';
  }
}

function getToolResultText(result: Message | undefined): string {
  if (!result) return '';
  const content = result.content as ToolResultContent | undefined;
  return content?.content || content?.result || content?.error || '';
}

function summarizeToolInput(name: string, input: Record<string, unknown>, display?: string): string {
  const formatted = formatToolInput(name, input, display).display.replace(/^\$\s*/, '').replace(/\s+/g, ' ').trim();
  return formatted.length > 120 ? `${formatted.slice(0, 119)}…` : formatted;
}

function buildToolResults(messages: Message[]): Map<string, Message> {
  const map = new Map<string, Message>();
  for (const msg of messages) {
    if (msg.message_type !== 'tool' && msg.type !== 'tool') continue;
    const content = msg.content as ToolResultContent;
    if (content?.tool_use_id) map.set(content.tool_use_id, msg);
  }
  return map;
}

function countToolUses(messages: Message[]): number {
  let count = 0;
  for (const msg of messages) {
    if (msg.message_type !== 'agent' && msg.type !== 'agent') continue;
    const blocks = Array.isArray(msg.content) ? (msg.content as ContentBlock[]) : [];
    count += blocks.filter((b) => b.type === 'tool_use').length;
  }
  return count;
}

function SubAgentStatusIcon({ status }: { status: SubAgentStatusKind }) {
  if (status === 'running') {
    return <span className="spinner"></span>;
  }
  if (status === 'success') return <CheckIcon />;
  return <XIcon />;
}

function ChildToolActivity({ block, result }: { block: ContentBlock; result: Message | undefined }) {
  const name = block.name || 'tool';
  const input = (block.input || {}) as Record<string, unknown>;
  const output = getToolResultText(result);
  const firstOutputLine = output.split('\n').find((line) => line.trim())?.trim() ?? '';
  const outputPreview = firstOutputLine ? truncate(firstOutputLine, 140) : result ? '(empty)' : 'running…';
  const outputClass = firstOutputLine ? '' : result ? 'empty' : 'pending';
  const isError = (result?.content as ToolResultContent | undefined)?.is_error || (result?.content as ToolResultContent | undefined)?.error;

  return (
    <div className={`subagent-activity-event tool ${isError ? 'error' : ''}`}>
      <span className="subagent-activity-tag">{name}</span>
      <code className="subagent-activity-command">{summarizeToolInput(name, input, block.display)}</code>
      <span className="subagent-activity-arrow">→</span>
      <span className={`subagent-activity-output ${outputClass}`} title={firstOutputLine || outputPreview}>
        {outputPreview}
      </span>
    </div>
  );
}

function ChildAgentActivity({ message, toolResults }: { message: Message; toolResults: Map<string, Message> }) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  return (
    <>
      {blocks.map((block, idx) => {
        if (block.type === 'text') {
          const text = block.text?.trim();
          if (!text) return null;
          return (
            <div key={`${message.message_id}-text-${idx}`} className="subagent-activity-event agent-text">
              <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>{text.length > 900 ? `${text.slice(0, 900)}…` : text}</ReactMarkdown>
            </div>
          );
        }
        if (block.type === 'tool_use') {
          return (
            <ChildToolActivity
              key={block.id || `${message.message_id}-tool-${idx}`}
              block={block}
              result={toolResults.get(block.id || '')}
            />
          );
        }
        return null;
      })}
    </>
  );
}

/**
 * Read-only sub-agent transcript, driven by the inline stream. Used both as
 * the inline peek inside the parent's `spawn_agents` card (truncated to the
 * latest steps) and, with `full`, as the body of the side-docked
 * `SubAgentViewerPanel` (every step, scrollable).
 */
export function ChildConversationActivity({ agentId, expanded, running, full = false }: { agentId: string; expanded: boolean; running: boolean; full?: boolean }) {
  const inline = useConversationInlineStream(agentId, expanded, running);

  if (!expanded) return null;

  const { atom } = inline;
  const toolResults = buildToolResults(atom.messages);
  const agentMessages = atom.messages.filter((m) => m.message_type === 'agent' || m.type === 'agent');
  const visibleAgentMessages = full ? agentMessages : agentMessages.slice(-12);
  const hiddenCount = Math.max(0, agentMessages.length - visibleAgentMessages.length);
  const toolCount = countToolUses(atom.messages);

  return (
    <div className="subagent-activity-panel">
      <div className="subagent-activity-meta">
        <span>{toolCount} tool{toolCount === 1 ? '' : 's'}</span>
        <span>{atom.messages.length} messages</span>
        {running && <span>live</span>}
      </div>
      {inline.type === 'connecting' && <div className="subagent-activity-placeholder">Loading sub-agent activity…</div>}
      {inline.type === 'error' && <div className="subagent-activity-error">{inline.error}</div>}
      {hiddenCount > 0 && (
        <div className="subagent-activity-placeholder">Showing latest {visibleAgentMessages.length} agent steps ({hiddenCount} earlier hidden)</div>
      )}
      {visibleAgentMessages.map((message) => (
        <ChildAgentActivity key={message.message_id} message={message} toolResults={toolResults} />
      ))}
      {atom.streamingBuffer?.text && (
        <div className="subagent-activity-event agent-text streaming">
          <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>{atom.streamingBuffer.text}</ReactMarkdown>
        </div>
      )}
      {inline.type !== 'connecting' && inline.type !== 'error' && visibleAgentMessages.length === 0 && !atom.streamingBuffer?.text && (
        <div className="subagent-activity-placeholder">No sub-agent activity yet.</div>
      )}
    </div>
  );
}

// Exported for regression testing of the docked-viewer state sync (see
// SubAgentViewerSync.test.tsx); rendered in-app only via SubAgentSummary.
export function SubAgentActivityCard({ agentId, task, outcome }: { agentId: string; task: string; outcome: SubAgentResult['outcome'] | null }) {
  const [expanded, setExpanded] = useState(false);
  const status = statusKindFromOutcome(outcome);
  const statusClass = status.replace('_', '-');
  const running = status === 'running';
  const resultText = outcome ? getOutcomeText(outcome) : '';
  const viewer = useSubAgentViewer();

  // Keep the docked viewer's record in sync with this card's live state. The
  // card re-renders as the sub-agent progresses (running → completed, empty →
  // final outcome); if this is the agent currently open in the panel, push the
  // new state so the panel stops streaming a finished agent and shows its final
  // outcome without a close/reopen. Guarded by an equality check so it
  // converges (the re-render triggered by `open` is a no-op on the next pass).
  const open = viewer?.open;
  const openedRecord = viewer?.opened;
  useEffect(() => {
    if (!open || openedRecord?.agentId !== agentId) return;
    if (
      openedRecord.running !== running ||
      openedRecord.resultText !== resultText ||
      openedRecord.task !== task
    ) {
      open({ agentId, task, running, resultText });
    }
  }, [open, openedRecord, agentId, task, running, resultText]);

  return (
    <div className={`subagent-item activity ${statusClass}`}>
      <div className="subagent-item-header">
        <button
          type="button"
          className="subagent-expand-button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          <span className="subagent-icon"><SubAgentStatusIcon status={status} /></span>
          <span className="subagent-label" title={task}>{truncate(task, 72)}</span>
          <span className="subagent-activity-count">activity</span>
          <span className={`subagent-status ${statusClass}`}>{getStatusLabel(status)}</span>
          <span className="subagent-expand-toggle">{expanded ? <ChevronUpIcon /> : <ChevronDownIcon />}</span>
        </button>
        <OpenConversationButton agentId={agentId} task={task} running={running} resultText={resultText} />
      </div>
      {resultText && !expanded && (
        <div className="subagent-result preview">{truncate(resultText, 180)}</div>
      )}
      <ChildConversationActivity agentId={agentId} expanded={expanded} running={running} />
      {expanded && resultText && (
        <div className={`subagent-final-result ${statusClass}`}>
          <div className="subagent-final-result-label">final outcome</div>
          <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>{resultText}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}

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
  return <SubAgentActivityCard agentId={result.agent_id} task={result.task} outcome={result.outcome} />;
}

/** Persistent summary of completed subagents (shown in spawn_agents tool result) */
function SubAgentSummary({ results }: { results: SubAgentResult[] }) {
  const successCount = results.filter(r => r.outcome.type === 'success').length;
  const timeoutCount = results.filter(r => r.outcome.type === 'timed_out').length;
  const failCount = results.filter(r => r.outcome.type === 'failure').length;

  return (
    <div className="subagent-summary-block">
      <div className="subagent-summary-title">
        <span className="subagent-summary-stats">
          {successCount > 0 && <span className="success"><CheckIcon /> {successCount}</span>}
          {failCount > 0 && <span className="error"><XIcon /> {failCount}</span>}
          {timeoutCount > 0 && <span className="error">⏱ {timeoutCount}</span>}
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

const ExternalLinkIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
    <polyline points="15 3 21 3 21 9" />
    <line x1="10" y1="14" x2="21" y2="3" />
  </svg>
);

/**
 * Resolves a sub-agent's slug and navigates to its full conversation page.
 * Sub-agent `agent_id` is the child conversation_id by construction
 * (runtime/executor.rs invariant); the route is keyed by slug, so resolve via
 * cacheDB (populated by the sidebar poll + SSE) with a REST fallback for the
 * rare cache miss. Hides itself only on a 404 (conversation deleted).
 *
 * This is the fallback path for `OpenConversationButton` — on desktop the
 * default action opens the sub-agent in the side panel instead.
 */
// eslint-disable-next-line react-refresh/only-export-components
export async function navigateToSubAgent(
  agentId: string,
  navigate: (path: string) => void,
): Promise<'ok' | 'missing'> {
  const cached = await cacheDB.getConversation(agentId);
  if (cached?.slug) {
    navigate(`/c/${cached.slug}`);
    return 'ok';
  }
  // Cache miss: ask the server. `getConversationSlug` returns null only for
  // 404 (conversation deleted) — that's the one case the caller hides the
  // button. Transient failures throw and leave the button in place.
  const slug = await api.getConversationSlug(agentId);
  if (slug) {
    navigate(`/c/${slug}`);
    return 'ok';
  }
  return 'missing';
}

/**
 * Opens a sub-agent's conversation. On desktop (where a `SubAgentViewerProvider`
 * is mounted) this docks the sub-agent in the side panel so the parent stays
 * visible; on mobile, or absent the provider, it falls back to navigating to
 * the sub-agent's full page.
 */
function OpenConversationButton({
  agentId,
  task,
  running,
  resultText,
}: {
  agentId: string;
  task: string;
  running: boolean;
  resultText: string;
}) {
  const navigate = useNavigate();
  const viewer = useSubAgentViewer();
  const isDesktop = useIsDesktop();
  const [busy, setBusy] = useState(false);
  const [missing, setMissing] = useState(false);
  // Synchronous guard against fast double-clicks. `busy` state lags by a
  // render so two clicks fired before React commits would both pass the
  // guard; a ref flips immediately.
  const inFlight = useRef(false);

  const usePanel = viewer !== null && isDesktop;

  const onClick = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (usePanel) {
      viewer.open({ agentId, task, running, resultText });
      return;
    }
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      if ((await navigateToSubAgent(agentId, navigate)) === 'missing') {
        setMissing(true);
      }
    } catch {
      // Transient error — keep the button enabled so the user can retry.
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }, [usePanel, viewer, agentId, task, running, resultText, navigate]);

  if (missing) return null;

  const label = usePanel ? 'Open sub-agent in side panel' : 'Open sub-agent conversation';
  return (
    <button
      type="button"
      className="subagent-open-link"
      onClick={onClick}
      title={label}
      aria-label={label}
      disabled={busy}
    >
      <ExternalLinkIcon />
    </button>
  );
}

type AwaitingSubAgentsState = Extract<ConversationState, { type: 'awaiting_sub_agents' }>;

export const SubAgentStatus = memo(SubAgentStatusImpl);

function SubAgentStatusImpl({ stateData }: { stateData: AwaitingSubAgentsState }) {
  const pending: PendingSubAgent[] = stateData.pending;
  const completed: SubAgentResult[] = stateData.completed_results;
  const total = pending.length + completed.length;
  const agents = [
    ...completed.map((result) => ({
      agentId: result.agent_id,
      task: result.task,
      outcome: result.outcome,
    })),
    ...pending.map((agent) => ({
      agentId: agent.agent_id,
      task: agent.task,
      outcome: null,
    })),
  ];

  return (
    <div className="subagent-status-block">
      <div className="subagent-header">
        <span className="subagent-title">Sub-agents</span>
        <span className="subagent-count">
          {completed.length}/{total}
        </span>
      </div>
      <div className="subagent-list">
        {agents.map((agent) => (
          <SubAgentActivityCard
            key={agent.agentId}
            agentId={agent.agentId}
            task={agent.task}
            outcome={agent.outcome}
          />
        ))}
      </div>
    </div>
  );
}
