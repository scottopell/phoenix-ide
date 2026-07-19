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
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import { api } from '../api';
import type { Message, ContentBlock, ToolResultContent, ConversationState, PendingSubAgent, SubAgentResult } from '../api';
import type { BashToolInput } from '../generated/sse';
import { cacheDB } from '../cache';
import type { QueuedMessage } from '../hooks';
import { useTheme } from '../hooks/useTheme';
import { useIsDesktop } from '../hooks';
import { useDensity } from '../hooks/useDensity';
import { useConversationInlineStream, type InlineStreamState } from '../hooks/useConversationInlineStream';
import { useSubAgentViewer } from '../contexts/SubAgentViewerContext';
import { useViewerSlotCommands } from '../contexts/ViewerSlotContext';

import { linkifyText } from '../utils/linkify';
import { getMessageMarkdown } from '../utils/messageCopy';
import { CopyButton } from './CopyButton';
import { PatchFileSummary, containsUnifiedDiff } from './PatchFileSummary';
import { BrowserProfileResponseView, STRUCTURED_PROFILE_ACTIONS } from './BrowserProfileResponseView';
import { deriveToolStripItems, type ToolStripItem } from './agentTurnToolStrip';
import { buildAgentTextFragments, buildKeywordSearchOutputProjection, buildMarkdownDisplayBlocks, buildPatchOutputProjection, buildReadFileOutputProjection, buildSearchOutputProjection, buildSubAgentCardFragments, buildTerminalToolResultProjection, type ConversationTextFragment, type ConversationFragmentRevealTarget, type TerminalToolResultFamily } from './viewer-find/searchProjections';
import { bashInputCopyText, cleanToolThoughts as cleanThoughts, formatToolInput, isBashToolInput, skillCommandFromInput, skillResultVisibleText, truncateToolInputValue as truncateValue } from './toolInputDisplay';
import { ForkProposalAffordance } from './ForkProposalAffordance';
import { ConversationMarkdownAnchor, ConversationMarkdownImage } from './conversationMarkdown';
import { CONVERSATION_MARKDOWN_COMPONENTS, CONVERSATION_MARKDOWN_URL_TRANSFORM, createConversationMarkdownComponents, resolveConversationMarkdownImageSrc } from './conversationMarkdownImages';
import { CommissionReviewInputView, CommissionReviewSummaryCard } from '../features/commissionReview/CommissionReviewSummary';
import { parseCommissionReviewInput, parseCommissionReviewResult } from '../features/commissionReview/model';
import { MermaidDiagram } from './MermaidDiagram';
import { StreamingBlocks } from './StreamingMessage';
import './ReadFileResultView.css';

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

// Inline-code / prose file-path links open the file with no modified-line
// selection. Share one empty Set rather than allocating a new one per markdown
// node on every render. Never mutated.
const EMPTY_LINE_SET = new Set<number>();

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

// =====================================================================// Helper functions
// =====================================================================
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

function parseSlashCommand(text: string): { token: string; args: string } | null {
  const match = text.trim().match(/^(\/[A-Za-z0-9][\w:-]*)(?:\s+([\s\S]*))?$/);
  if (!match) return null;
  return { token: match[1] ?? '/skill', args: match[2] ?? '' };
}

function skillTitle(token: string, source?: string, snippet?: string): string {
  return [
    `Skill invocation: ${token}`,
    source ? `Source: ${source}` : '',
    snippet ? `Preview: ${snippet}` : '',
  ].filter(Boolean).join('\n');
}

export function SkillCommandText({
  text,
  source,
  snippet,
}: {
  text: string;
  source?: string | undefined;
  snippet?: string | undefined;
}) {
  const parsed = parseSlashCommand(text);
  if (!parsed) return <>{text}</>;
  return (
    <span className="skill-command-inline">
      <span className="skill-command-chip" title={skillTitle(parsed.token, source, snippet)}>
        <span className="skill-command-slash">/</span>
        <span className="skill-command-name">{parsed.token.slice(1)}</span>
      </span>
      {parsed.args && <span className="skill-command-args"> {parsed.args}</span>}
    </span>
  );
}

function extractSkillResultDetails(resultText: string): { source?: string | undefined; snippet?: string | undefined } {
  const source = resultText.match(/^Base directory for this skill:\s*(.+)$/m)?.[1]?.trim();
  const contentLines = resultText
    .split('\n')
    .filter(line => line.trim() && !line.startsWith('Base directory for this skill:'));
  const snippet = contentLines.find(line => line.startsWith('# ')) || contentLines[0] || '';
  return {
    source,
    snippet: snippet ? truncateValue(snippet.replace(/^#\s*/, '').trim(), 120) : undefined,
  };
}

function SkillToolBlock({
  input,
  resultText,
  result,
  isError,
  durationMs,
  toolStartedAtMs,
  inflightElapsedSeconds,
  onOpenFile,
  toolId,
  activeHighlight,
  inputActiveHighlight,
}: {
  input: Record<string, unknown>;
  resultText: string;
  result: Message | undefined;
  isError: boolean;
  durationMs: number | undefined;
  toolStartedAtMs: number | null | undefined;
  inflightElapsedSeconds: number;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  toolId: string;
  activeHighlight?: AgentTextHighlight | null;
  inputActiveHighlight?: AgentTextHighlight | null;
}) {
  const details = extractSkillResultDetails(resultText);
  const sourcePath = details.source ? `${details.source}/SKILL.md` : undefined;
  const status = result == null ? 'loading' : isError ? 'failed' : 'loaded';
  const statusClass = result == null ? 'pending' : isError ? 'error' : 'success';

  const inputText = skillCommandFromInput(input);

  if (activeHighlight) {
    if (isError) {
      return (
        <div className={`tool-block skill-tool-block ${statusClass}`} data-tool-id={toolId}>
          <TerminalToolResultHighlight semanticText={resultText} fragmentId={activeHighlight.fragmentId} activeHighlight={activeHighlight} />
        </div>
      );
    }
    const visibleResult = skillResultVisibleText(resultText);
    return (
      <div className={`tool-block skill-tool-block ${statusClass}`} data-tool-id={toolId}>
        <div className="skill-tool-status-row" data-fragment-id={activeHighlight.fragmentId}>
          {renderHighlightedText(visibleResult, activeHighlight.start, activeHighlight.end)}
        </div>
      </div>
    );
  }

  return (
    <div className={`tool-block skill-tool-block ${statusClass}`} data-tool-id={toolId}>
      <div className="skill-tool-status-row">
        <span data-fragment-id="tool-use-input">
          {inputActiveHighlight
            ? renderHighlightedText(inputText, inputActiveHighlight.start, inputActiveHighlight.end)
            : <SkillCommandText text={inputText} source={sourcePath} snippet={details.snippet} />}
        </span>
        <span className={`skill-tool-status ${statusClass}`}>
          {status}
          {durationMs !== undefined && <span className="tool-block-duration">&bull; {formatToolDuration(durationMs)}</span>}
          {result == null && toolStartedAtMs != null && <span className="tool-block-duration">&bull; {inflightElapsedSeconds}s</span>}
        </span>
      </div>
      {(sourcePath || details.snippet) && (
        <div className="skill-tool-detail-row">
          {sourcePath && (
            onOpenFile ? (
              <button
                type="button"
                className="skill-source-link"
                onClick={() => onOpenFile(sourcePath, new Set(), 0)}
                title={`Open ${sourcePath}`}
              >
                SKILL.md
              </button>
            ) : (
              <span className="skill-source-link static" title={sourcePath}>SKILL.md</span>
            )
          )}
          {details.snippet && <span className="skill-tool-snippet">{details.snippet}</span>}
        </div>
      )}
    </div>
  );
}

// =====================================================================// User Message Components
// =====================================================================
function MessageCopyButton({ message, title }: { message: Message; title: string }) {
  const markdown = getMessageMarkdown(message);
  if (markdown.trim() === '') return null;

  return (
    <CopyButton
      text={markdown}
      className="message-mobile-copy"
      title={title}
    />
  );
}

function formatAttachmentBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function FileChips({
  files,
  activeHighlight = null,
}: {
  files: { original_name: string; size_bytes: number; stored_path?: string }[];
  activeHighlight?: ConversationHighlight | null;
}) {
  if (files.length === 0) return null;
  return (
    <div className="message-files">
      {files.map((file, idx) => (
        <span
          key={`${file.stored_path ?? file.original_name}-${idx}`}
          className="message-file-chip"
          title={file.stored_path}
          data-fragment-id={`message-attachment-${idx}`}
        >
          📎 {activeHighlight?.owner === 'message-attachment' && activeHighlight.fragmentId === `message-attachment-${idx}`
            ? renderHighlightedText(file.original_name, activeHighlight.start, activeHighlight.end)
            : file.original_name}{' '}
          <span className="message-file-size">{formatAttachmentBytes(file.size_bytes)}</span>
        </span>
      ))}
    </div>
  );
}

export const UserMessage = memo(UserMessageImpl);

function UserMessageImpl({ message, activeHighlight = null }: { message: Message; activeHighlight?: ConversationHighlight | null }) {
  const content = message.content as { text?: string; images?: { data: string; media_type: string }[]; files?: { original_name: string; size_bytes: number; stored_path?: string }[]; is_meta?: boolean };
  const text = content.text || (typeof message.content === 'string' ? message.content : '');
  const images = content.images || [];
  const files = content.files || [];
  const isMeta = content.is_meta === true;
  const timestamp = message.created_at;

  return (
    <div id={`message-${message.message_id}`} className={`message ${isMeta ? 'meta' : 'user'}`} data-sequence-id={message.sequence_id}>
      <div className="message-header">
        <span className="message-header-meta">
          {!isMeta && <span className="message-sender">You</span>}
          {timestamp && (
            <span className="message-time" title={new Date(timestamp).toLocaleString()}>
              {formatMessageTime(timestamp)}
            </span>
          )}
          {!isMeta && <span className="message-status sent" title="Sent">&#x2713;</span>}
        </span>
        <span className="message-header-actions">
          <MessageCopyButton message={message} title="Copy your message" />
        </span>
      </div>
      <div className="message-content">
        <span data-fragment-id="message-text">
          {activeHighlight?.owner === 'message-text'
            ? renderHighlightedText(text, activeHighlight.start, activeHighlight.end)
            : <SkillCommandText text={text} />}
        </span>
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
        <FileChips files={files} activeHighlight={activeHighlight} />
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
  activeHighlight = null,
}: {
  message: QueuedMessage;
  onRetry: (localId: string) => void;
  onCancelSteering?: ((localId: string) => void) | undefined;
  activeHighlight?: ConversationHighlight | null;
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
        <span data-fragment-id="message-text">
          {activeHighlight?.owner === 'message-text'
            ? renderHighlightedText(message.text, activeHighlight.start, activeHighlight.end)
            : <SkillCommandText text={message.text} />}
        </span>
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
        <FileChips files={message.files ?? []} activeHighlight={activeHighlight} />
      </div>
    </div>
  );
}

type MarkdownHighlight = {
  sourceStart: number;
  sourceEnd: number;
  start: number;
  end: number;
};

type HastNode = {
  type?: string;
  value?: string;
  position?: { start?: { offset?: number }; end?: { offset?: number } };
  children?: HastNode[];
  tagName?: string;
  properties?: Record<string, unknown>;
};

function markdownHighlightForRange(markdown: string, start: number, end: number): MarkdownHighlight | null {
  let blockStart = 0;
  for (const block of buildMarkdownDisplayBlocks(markdown).filter((block) => block.kind !== 'code' || block.language === 'mermaid')) {
    const blockEnd = blockStart + block.searchableText.length;
    if (start >= blockStart && end <= blockEnd) {
      return {
        sourceStart: block.sourceRange.start,
        sourceEnd: block.sourceRange.end,
        start: start - blockStart,
        end: end - blockStart,
      };
    }
    blockStart = blockEnd + 1;
  }
  return null;
}

function markdownHighlightPlugin(highlight: MarkdownHighlight) {
  return () => (tree: HastNode) => {
    const target = findMarkdownHighlightTarget(tree, highlight);
    if (target?.tagName === 'code') return;
    if (target) decorateMarkdownTextNodes(target, highlight.start, highlight.end);
  };
}

function findMarkdownHighlightTarget(node: HastNode, highlight: MarkdownHighlight): HastNode | null {
  const nodeStart = node.position?.start?.offset;
  const nodeEnd = node.position?.end?.offset;
  const containsSourceRange = nodeStart !== undefined
    && nodeEnd !== undefined
    && nodeStart <= highlight.sourceStart
    && nodeEnd >= highlight.sourceEnd;
  if (!containsSourceRange) return null;
  for (const child of node.children ?? []) {
    if (!child.children) continue;
    const target = findMarkdownHighlightTarget(child, highlight);
    if (target) return target;
  }
  return node.children ? node : null;
}

function decorateMarkdownTextNodes(node: HastNode, start: number, end: number): void {
  let cursor = 0;
  const visit = (parent: HastNode) => {
    const nextChildren: HastNode[] = [];
    for (const child of parent.children ?? []) {
      if (child.type !== 'text' || typeof child.value !== 'string') {
        visit(child);
        nextChildren.push(child);
        continue;
      }
      const childStart = cursor;
      const childEnd = childStart + child.value.length;
      const overlapStart = Math.max(start, childStart);
      const overlapEnd = Math.min(end, childEnd);
      if (overlapStart < overlapEnd) {
        const localStart = overlapStart - childStart;
        const localEnd = overlapEnd - childStart;
        if (localStart > 0) nextChildren.push({ type: 'text', value: child.value.slice(0, localStart) });
        nextChildren.push({
          type: 'element',
          tagName: 'mark',
          properties: { className: ['viewer-find-inline-match', 'viewer-find-inline-match--active'] },
          children: [{ type: 'text', value: child.value.slice(localStart, localEnd) }],
        });
        if (localEnd < child.value.length) nextChildren.push({ type: 'text', value: child.value.slice(localEnd) });
      } else {
        nextChildren.push(child);
      }
      cursor = childEnd;
    }
    if (parent.children) parent.children = nextChildren;
  };
  visit(node);
}

// eslint-disable-next-line react-refresh/only-export-components
export function renderHighlightedText(text: string, start: number, end: number): React.ReactNode {
  if (start < 0 || end <= start || start >= text.length) return text;
  return (
    <>
      {text.slice(0, start)}
      <mark className="viewer-find-inline-match viewer-find-inline-match--active">{text.slice(start, Math.min(end, text.length))}</mark>
      {text.slice(Math.min(end, text.length))}
    </>
  );
}

/**
 * A fully-rendered assistant prose block. Memoized so a turn's completed
 * prose is not re-parsed through ReactMarkdown every time the parent
 * AgentMessage re-renders — which happens repeatedly during an active turn
 * as each tool result lands (the `toolResults` Map gets a new identity).
 * `remarkPlugins` is one of two module-level constants and `components` is
 * a memoized map, so a shallow prop compare bails for unchanged text.
 * Mirrors the per-block memoization StreamingMessage uses for the same reason.
 */
/**
 * An assistant text block that, in compact mode, has content hidden by its
 * preview. Renders as a faded clickable one-liner that expands to the full
 * markdown on click — never destructive, the full text is always one click
 * away (and the title attr carries the first line for hover).
 */
const CollapsibleText = memo(CollapsibleTextImpl);

function CollapsibleTextImpl({
  text,
  summary,
  expanded,
  onExpand,
}: {
  text: string;
  summary: string;
  expanded: boolean;
  onExpand: () => void;
}) {
  if (expanded) {
    return <div className="agent-text-block">{text}</div>;
  }

  return (
    <div
      className="agent-text-collapsed"
      role="button"
      tabIndex={0}
      title={summary}
      onClick={onExpand}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onExpand();
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
 * results (via `deriveToolStripItems`), never from phase state.
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
  const hasRunningTimer = items.some((item) => !item.hasResult && item.startedAtMs !== null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    if (!hasRunningTimer) return undefined;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [hasRunningTimer]);

  return (
    <div className="compact-tool-strip" role="list" aria-label="Tool calls">
      {items.map((item, i) => {
        const classNames = [
          'compact-tool-card',
          item.isSubAgent ? 'subagents' : '',
          item.isError ? 'error' : '',
          !item.hasResult ? 'pending' : '',
        ].filter(Boolean).join(' ');
        const summary = item.resultSummary ?? item.inputSummary;
        const statusLabel = item.isError
          ? 'failed'
          : item.hasResult
            ? 'done'
            : item.startedAtMs !== null || item.finalStatus === 'running'
              ? 'running'
              : 'queued';
        const liveElapsed = !item.hasResult && item.startedAtMs !== null
          ? ` · ${Math.max(0, Math.floor((nowMs - item.startedAtMs) / 1_000))}s`
          : '';
        const ariaStatus = item.finalStatus ?? statusLabel;
        const ariaSummary = item.outputTail ?? summary;
        const isCompactBash = item.name === 'bash';
        return (
          <button
            key={item.toolId || `${item.name}-${i}`}
            type="button"
            className={classNames}
            onClick={() => onExpand(item.toolId)}
            aria-label={`${item.name}: ${item.commandIdentity ?? item.inputSummary} (${ariaStatus})${ariaSummary ? ` — ${ariaSummary}` : ''} — expand tool detail`}
          >
            <span className="compact-tool-card-header">
              <span className="compact-tool-card-name">{item.name}</span>
              <span className="compact-tool-card-status">{item.finalStatus ?? statusLabel}{liveElapsed}</span>
            </span>
            {isCompactBash ? (
              <>
                <span className="compact-tool-card-identity" title={item.commandIdentity ?? item.inputSummary}>
                  {item.commandIdentity ?? item.inputSummary}
                </span>
                <span className="compact-tool-card-summary compact-tool-card-summary-tail" title={item.outputTail ?? ''}>
                  {item.outputTail ?? '(no output)'}
                </span>
              </>
            ) : (
              <span className="compact-tool-card-summary" title={summary}>{summary}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}

// =====================================================================// Agent Message Components
// =====================================================================
export interface AgentTextRevealRequest {
  unitKey: string;
  fragmentId: string;
  revealTarget: ConversationFragmentRevealTarget;
  nonce: number;
}

export interface AgentTextHighlight {
  fragmentId: string;
  start: number;
  end: number;
}

export type ConversationHighlight = AgentTextHighlight & (
  | { owner: 'message-text' }
  | { owner: 'message-attachment' }
  | { owner: 'agent-text' }
  | { owner: 'tool-input'; toolUseId: string }
  | { owner: 'tool-result'; toolUseId: string }
);

interface AgentMessageProps {
  message: Message;
  toolResults: ReadonlyMap<string, Message>;
  onOpenFile?: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  onOpenCommissionReview?: ((requestSequenceId: number) => void) | undefined;
  filePathRootDir?: string | undefined;
  workScopeKey?: string | undefined;
  activeToolUseId?: string | undefined;
  liveBashProgress?: import('../conversation/atom').ConversationAtom['liveBashProgress'];
  /**
   * When false, suppresses the "Phoenix HH:MM" header row. Used by the list
   * to collapse repeated headers across a run of consecutive agent messages
   * within the same turn. Defaults to true so callers that don't set it keep
   * the original behavior.
   */
  isFirstInTurn?: boolean;
  /**
   * When true, the compact-density prose preview is bypassed so the message
   * renders in its full non-collapsed form.
   */
  forceExpandedText?: boolean;
  isLatestAgentMessage?: boolean;
  unitKey?: string;
  revealRequest?: AgentTextRevealRequest | null;
  activeHighlight?: ConversationHighlight | null;
  onRevealHandled?: ((request: AgentTextRevealRequest) => void) | undefined;
}

export const AgentMessage = memo(AgentMessageImpl);

function AgentMessageImpl({ message, toolResults, onOpenFile, onOpenCommissionReview, filePathRootDir, workScopeKey, activeToolUseId, liveBashProgress = {}, isFirstInTurn = true, forceExpandedText = false, isLatestAgentMessage = false, unitKey, revealRequest = null, activeHighlight = null, onRevealHandled }: AgentMessageProps) {
  const blocks = useMemo(
    () => (Array.isArray(message.content) ? (message.content as ContentBlock[]) : []),
    [message.content],
  );
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
    () => deriveToolStripItems(message, toolResults, liveBashProgress),
    [message, toolResults, liveBashProgress],
  );
  const knownResultIds = useMemo(
    () => (import.meta.env.DEV ? Array.from(toolResults.keys()) : undefined),
    [toolResults],
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

  useEffect(() => {
    if (!compact || toolsExpanded || !revealRequest || revealRequest.revealTarget.kind === 'agent-text') return;
    pendingScrollToolIdRef.current = 'toolUseId' in revealRequest.revealTarget
      ? revealRequest.revealTarget.toolUseId
      : null;
    setToolsExpanded(true);
  }, [compact, revealRequest, toolsExpanded]);

  const filePathCopyContext = useMemo(
    () => (filePathRootDir ? { rootDir: filePathRootDir } : undefined),
    [filePathRootDir],
  );


  // Stable markdown component map — only recreated when onOpenFile identity changes.
  // Keeps ReactMarkdown from remounting SyntaxHighlighter on every parent re-render.
  const markdownComponents = useMemo(() => {
    // One handler shared by every code/paragraph/list node, instead of a fresh
    // closure (and Set) allocated per node on every render.
    const fileClickHandler = onOpenFile
      ? (filePath: string) => onOpenFile(filePath, EMPTY_LINE_SET, 0)
      : undefined;
    const processChildren = (nodes: React.ReactNode): React.ReactNode[] => {
      return React.Children.toArray(nodes).flatMap((child) => {
        if (typeof child === 'string') {
          return linkifyText(child, fileClickHandler, filePathCopyContext);
        }
        return child;
      });
    };
    return {
      // Custom code block rendering with syntax highlighting
      // Inline code with file paths becomes clickable
      code: ({ inline, className, children, node, ...props }: { inline?: boolean | undefined; className?: string | undefined; children?: React.ReactNode; node?: unknown }) => {
        void node;
        const match = /language-([^\s]+)/.exec(className || '');
        const language = match?.[1]?.toLowerCase();
        if (!inline && language === 'mermaid') {
          return <MermaidDiagram code={String(children)} />;
        }
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
      p: ({ children }: { children?: React.ReactNode }) => <p>{processChildren(children)}</p>,
      // Custom list item rendering with clickable file paths
      li: ({ children }: { children?: React.ReactNode }) => <li>{processChildren(children)}</li>,
      a: (props) => (
        <ConversationMarkdownAnchor
          {...props}
          onFileClick={fileClickHandler}
          filePathCopyContext={filePathCopyContext}
        />
      ),
      table: MarkdownTable,
      img: ({ src, ...props }: React.ComponentPropsWithoutRef<'img'> & { node?: unknown }) => (
        <ConversationMarkdownImage
          {...props}
          src={resolveConversationMarkdownImageSrc(src, filePathRootDir)}
        />
      ),
    } satisfies Components;
  }, [onOpenFile, filePathCopyContext, filePathRootDir, syntaxStyle]);

  const textFragments = useMemo(
    () => buildAgentTextFragments(blocks, density, { forceExpandedText }),
    [blocks, density, forceExpandedText],
  );
  const [expandedFragmentIds, setExpandedFragmentIds] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    if (!revealRequest || revealRequest.unitKey !== unitKey) return;
    if (revealRequest.revealTarget.kind !== 'agent-text') return;
    setExpandedFragmentIds((current) => {
      if (current.has(revealRequest.fragmentId)) return current;
      const next = new Set(current);
      next.add(revealRequest.fragmentId);
      return next;
    });
    onRevealHandled?.(revealRequest);
  }, [onRevealHandled, revealRequest, unitKey]);

  const expandFragment = useCallback((fragmentId: string) => {
    setExpandedFragmentIds((current) => {
      if (current.has(fragmentId)) return current;
      const next = new Set(current);
      next.add(fragmentId);
      return next;
    });
  }, []);

  const renderTextFragment = useCallback((fragment: ConversationTextFragment) => {
    const remarkPlugins = usesGfmSyntax(fragment.display.sourceText) ? REMARK_PLUGINS : NO_REMARK_PLUGINS;
    const expanded = forceExpandedText || fragment.display.mode === 'full' || expandedFragmentIds.has(fragment.fragmentId);
    const highlight = activeHighlight?.owner === 'agent-text' && activeHighlight.fragmentId === fragment.fragmentId
      ? activeHighlight
      : null;
    const markdownHighlight = highlight
      ? markdownHighlightForRange(fragment.display.sourceText, highlight.start, highlight.end)
      : null;

    if (!expanded) {
      return (
        <CollapsibleText
          key={fragment.fragmentId}
          text={fragment.display.sourceText}
          summary={fragment.display.summaryText}
          expanded={false}
          onExpand={() => expandFragment(fragment.fragmentId)}
        />
      );
    }
    return (
      <div key={fragment.fragmentId} className="agent-text-fragment" data-fragment-id={fragment.fragmentId}>
        <div className="agent-text-block" {...(highlight ? { 'data-active-fragment-highlight': true } : {})}>
          <ReactMarkdown
            remarkPlugins={remarkPlugins}
            rehypePlugins={markdownHighlight ? [markdownHighlightPlugin(markdownHighlight)] : []}
            components={markdownComponents}
            urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}
          >
            {fragment.display.sourceText}
          </ReactMarkdown>
        </div>
      </div>
    );
  }, [activeHighlight, expandFragment, expandedFragmentIds, forceExpandedText, markdownComponents]);
  const textFragmentById = useMemo(
    () => new Map(textFragments.map((fragment) => [fragment.fragmentId, fragment])),
    [textFragments],
  );

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
    <div id={`message-${message.message_id}`} className="message agent" data-sequence-id={message.sequence_id}>
      {!isFirstInTurn && (
        <div className="message-mobile-copy-row">
          <MessageCopyButton message={message} title="Copy Phoenix message" />
        </div>
      )}
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
          <span className="message-header-actions">
            <MessageCopyButton message={message} title="Copy Phoenix message" />
          </span>
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
              const fragment = textFragmentById.get(`agent-text-${i}`);
              if (!fragment || fragment.semanticText.trim() === '') {
                return null;
              }
              return renderTextFragment(fragment);
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
              const toolUseId = block.id;
              const toolStartedAtMs =
                toolUseId !== undefined &&
                  toolUseId === activeToolUseId &&
                  toolStartsMap &&
                  typeof toolStartsMap[toolUseId] === 'number'
                  ? (toolStartsMap[toolUseId] as number)
                  : undefined;
              const result = toolResults.get(block.id || '');
              const showMissingResult =
                result === undefined && !(isLatestAgentMessage && activeToolUseId !== undefined);
              return (
                  <ToolUseBlock
                    key={block.id || i}
                    block={block}
                    result={result}
                    onOpenFile={onOpenFile}
                    onOpenCommissionReview={onOpenCommissionReview}
                    requestSequenceId={message.sequence_id}
                    workScopeKey={workScopeKey}
                    knownResultIds={knownResultIds}
                    toolStartedAtMs={toolStartedAtMs}
                    showMissingResult={showMissingResult}
                    liveBashProgress={liveBashProgress[block.id || '']?.progress}
                    revealRequest={revealRequest}
                    activeHighlight={activeHighlight}
                    onRevealHandled={onRevealHandled}
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

// =====================================================================// Think Aside — subtle inline collapsed aside for `think` tool blocks
// =====================================================================
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

// =====================================================================// Tool Use Block
// =====================================================================
interface ToolUseBlockProps {
  block: ContentBlock;
  result: Message | undefined;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  onOpenCommissionReview?: ((requestSequenceId: number) => void) | undefined;
  requestSequenceId?: number | undefined;
  workScopeKey?: string | undefined;
  knownResultIds?: readonly string[] | undefined;
  revealRequest?: AgentTextRevealRequest | null;
  activeHighlight?: ConversationHighlight | null;
  onRevealHandled?: ((request: AgentTextRevealRequest) => void) | undefined;
  /** Server-clock unix ms when the runtime began dispatching this
   *  tool — sourced from the parent assistant message's
   *  `display_data.tool_starts[block.id]` (REQ-WPV-002). When present
   *  and no `result` has landed yet, the tool widget renders a live
   *  elapsed counter that survives reconnect / reload / multi-tab. */
  toolStartedAtMs?: number | undefined;
  showMissingResult?: boolean | undefined;
  liveBashProgress?: import('../generated/sse').BashToolProgress | undefined;
}

type ToolCardState =
  | { kind: 'declared' }
  | { kind: 'running'; toolStartedAtMs: number }
  | { kind: 'completed'; result: Message; resultContent: ToolResultContent; durationMs?: number }
  | { kind: 'failed'; result: Message; resultContent: ToolResultContent; durationMs?: number }
  | { kind: 'missing_result' };

function getToolCardState({
  toolId,
  result,
  toolStartedAtMs,
}: {
  toolId: string;
  result: Message | undefined;
  toolStartedAtMs?: number | undefined;
}): ToolCardState {
  if (result) {
    const resultContent = result.content as ToolResultContent;
    const dd = result.display_data as Record<string, unknown> | undefined;
    const durationValue = dd?.['duration_ms'];
    const durationMs = typeof durationValue === 'number' ? durationValue : undefined;
    const isError = resultContent?.is_error || !!resultContent?.error;
    const duration = durationMs !== undefined ? { durationMs } : {};
    return isError
      ? { kind: 'failed', result, resultContent, ...duration }
      : { kind: 'completed', result, resultContent, ...duration };
  }
  if (toolStartedAtMs != null) {
    return { kind: 'running', toolStartedAtMs };
  }
  if (toolId) {
    return { kind: 'missing_result' };
  }
  return { kind: 'declared' };
}

function logMissingToolResult({
  toolId,
  name,
  knownResultIds,
  toolStartedAtMs,
}: {
  toolId: string;
  name: string;
  knownResultIds?: readonly string[] | undefined;
  toolStartedAtMs?: number | undefined;
}): void {
  if (!import.meta.env.DEV) return;
  console.debug('[MessageComponents] rendering missing tool result', {
    tool_use_id: toolId || null,
    tool_call_id: toolId || null,
    tool_name: name,
    tool_started_at_ms: toolStartedAtMs ?? null,
    known_result_ids: knownResultIds ?? [],
    known_result_count: knownResultIds?.length ?? 0,
  });
}

function renderToolCardState(state: ToolCardState, inflightElapsedSeconds: number): React.ReactNode {
  switch (state.kind) {
    case 'declared':
      return <span className="tool-block-status pending">Declared</span>;
    case 'running':
      return (
        <span
          className="tool-block-elapsed"
          title={`Started ${new Date(state.toolStartedAtMs).toLocaleTimeString()}`}
        >
          &bull; {inflightElapsedSeconds}s
        </span>
      );
    case 'completed':
      return (
        <span className="tool-block-status success">
          <CheckIcon />
          {state.durationMs !== undefined && (
            <span className="tool-block-duration">&bull; {formatToolDuration(state.durationMs)}</span>
          )}
        </span>
      );
    case 'failed':
      return (
        <span className="tool-block-status error">
          <XIcon />
          {state.durationMs !== undefined && (
            <span className="tool-block-duration">&bull; {formatToolDuration(state.durationMs)}</span>
          )}
        </span>
      );
    case 'missing_result':
      return <span className="tool-block-status pending">Waiting for tool result</span>;
    default:
      state satisfies never;
      return null;
  }
}

function renderMissingToolResultBody(state: ToolCardState): React.ReactNode {
  switch (state.kind) {
    case 'declared':
      return <div className="tool-block-output-content"><span className="tool-empty">Tool declared</span></div>;
    case 'running':
      return <div className="tool-block-output-content tool-missing-result">result not received</div>;
    case 'missing_result':
      return <div className="tool-block-output-content tool-missing-result">result not received</div>;
    case 'completed':
    case 'failed':
      return null;
    default:
      state satisfies never;
      return null;
  }
}

function getToolResultTextFromContent(resultContent: ToolResultContent | null): string {
  return resultContent?.content || resultContent?.result || resultContent?.error || '';
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

function BashInspectButton({ workScopeKey, handle }: { workScopeKey: string; handle: string }) {
  const { openInspect } = useViewerSlotCommands();
  return (
    <button
      type="button"
      className="bash-inspect"
      onClick={() => openInspect(workScopeKey, handle)}
      title="Open the process inspector for this handle"
    >
      inspect →
    </button>
  );
}

// REQ-BASH-002 / REQ-BASH-003 / REQ-BASH-006: render the typed bash tool
// response. Renders a status pill, optional kill-pending badge, the line
// tail, and (when present) the agent-supplied `label` so concurrent
// handles are distinguishable at a glance.
function bashStatusText(status: string, finalCause: string | null): string {
  switch (status) {
    case 'running': return 'running';
    case 'still_running': return 'still running';
    case 'kill_pending_kernel': return 'kill pending (kernel)';
    case 'tombstoned': return finalCause ? `tombstoned · ${finalCause}` : 'tombstoned';
    case 'exited': return 'exited';
    case 'killed': return 'killed';
    default: return status;
  }
}

function formatBashMillis(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(1)}s`;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.round(seconds % 60);
  return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
}

function summarizeBashVisibleTail(lines: string[], partial: string | null, truncatedBefore: boolean, maxLines = 8): string[] {
  const out = lines.slice(-Math.max(0, maxLines - (partial ? 1 : 0)));
  if (partial) out.push(partial);
  if (truncatedBefore && out.length > 0) out[0] = `… ${out[0]}`;
  return out;
}

function BashOutputView({
  lines,
  partial,
  truncatedBefore,
  bounded = false,
}: {
  lines: string[];
  partial: string | null;
  truncatedBefore: boolean;
  bounded?: boolean;
}) {
  const visible = bounded
    ? summarizeBashVisibleTail(lines, partial, truncatedBefore)
    : [...lines, ...(partial ? [partial] : [])];
  const localTailTruncated = bounded && lines.length + (partial ? 1 : 0) > 8;
  const outputTruncated = truncatedBefore || localTailTruncated;
  if (visible.length === 0) return null;
  return (
    <div className="bash-output-shell">
      {outputTruncated && (
        <div className="bash-truncated-notice" aria-label="Earlier bash output fell out of the bounded tail">
          [older output omitted from this tail]
        </div>
      )}
      <div className="bash-output-viewport" role="log" aria-live="polite" aria-atomic="false" aria-relevant="additions text">
        {visible.map((line, index) => {
          const isPartial = partial !== null && index === visible.length - 1;
          return (
            <div
              key={`${index}-${line}`}
              className={`bash-output-line${isPartial ? ' bash-output-line-partial' : ''}`}
              title={isPartial ? 'Live trailing partial line — still being written' : undefined}
            >
              <span className="bash-output-line-text">{line}</span>
              {isPartial && <span className="bash-partial-badge">partial</span>}
            </div>
          );
        })}
      </div>
      {partial && <div className="bash-partial-affordance">[final line still streaming — no newline yet]</div>}
    </div>
  );
}

function BashResponseView({ response, workScopeKey }: { response: Record<string, unknown>; workScopeKey?: string | undefined }) {
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
  const partial = typeof response['partial'] === 'string' ? response['partial'] : null;
  const lineTexts = lines.map((l) => l.bytes ?? '');
  const statusText = bashStatusText(status, finalCause);

  return (
    <div className="bash-response">
      <div className="bash-response-header">
        <span className={`bash-status bash-status-${status.replace(/_/g, '-')}`}>{statusText}</span>
        {handle && <span className="bash-handle">{handle}</span>}
        {handle && workScopeKey && <BashInspectButton workScopeKey={workScopeKey} handle={handle} />}
        {label && <span className="bash-label" title="agent-supplied handle label">{label}</span>}
        {(isExited || isTombstone) && exitCode !== undefined && exitCode !== null && (
          <span className="bash-exit-code">exit {String(exitCode)}</span>
        )}
        {(isKilled || isTombstone) && typeof signalNumber === 'number' && (
          <span className="bash-signal-number">signal {String(signalNumber)}</span>
        )}
        {killSignalSent && <span className="bash-kill-signal">kill {killSignalSent}</span>}
        {signalSent && signalSent !== killSignalSent && <span className="bash-signal-sent">sent {signalSent}</span>}
        {waitedMs !== null && <span className="bash-duration">waited {formatBashMillis(waitedMs)}</span>}
        {durationMs !== null && <span className="bash-duration">ran {formatBashMillis(durationMs)}</span>}
      </div>
      <BashOutputView lines={lineTexts} partial={partial} truncatedBefore={truncatedBefore} />
      {status === 'waiter_panicked' && typeof response['error_message'] === 'string' && (
        <div className="bash-error-message">{response['error_message']}</div>
      )}
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

type ReadFileRequest = {
  path: string;
  offset: number | null;
  limit: number | null;
};

type ReadFileLine = {
  lineNumber: number;
  content: string;
};

type ReadFileParseResult = {
  lines: ReadFileLine[];
  notes: string[];
  malformed: boolean;
};

type ReadFileDisplayData = {
  type: 'read_file';
  path: string;
  requested_offset: number;
  requested_limit: number;
  returned_start_line: number | null;
  returned_end_line: number | null;
  returned_line_count: number;
  total_line_count: number;
  remaining_line_count: number;
  viewer_available: boolean;
};

function parseReadFileDisplayData(value: unknown): ReadFileDisplayData | null {
  if (!value || typeof value !== 'object') return null;
  const data = value as Record<string, unknown>;
  if (data['type'] !== 'read_file' || typeof data['path'] !== 'string') return null;
  if (typeof data['viewer_available'] !== 'boolean') return null;
  const numericKeys = ['requested_offset', 'requested_limit', 'returned_line_count', 'total_line_count', 'remaining_line_count'] as const;
  if (numericKeys.some((key) => !Number.isInteger(data[key]) || (data[key] as number) < 0)) return null;
  const validLine = (key: 'returned_start_line' | 'returned_end_line') =>
    data[key] === null || (Number.isInteger(data[key]) && (data[key] as number) > 0);
  if (!validLine('returned_start_line') || !validLine('returned_end_line')) return null;
  return data as ReadFileDisplayData;
}

const READ_FILE_PREVIEW_MAX_LINES = 20;
const READ_FILE_PREVIEW_MAX_CHARS = 5_000;

function boundedReadFileLines(lines: ReadFileLine[]): { lines: ReadFileLine[]; truncated: boolean } {
  const visible: ReadFileLine[] = [];
  let remainingChars = READ_FILE_PREVIEW_MAX_CHARS;
  for (const line of lines.slice(0, READ_FILE_PREVIEW_MAX_LINES)) {
    if (remainingChars <= 0) break;
    const lineWasTruncated = line.content.length > remainingChars;
    const content = lineWasTruncated
      ? `${line.content.slice(0, remainingChars)}…`
      : line.content;
    visible.push({ ...line, content });
    remainingChars -= Math.min(line.content.length, remainingChars);
    if (lineWasTruncated) break;
  }
  return {
    lines: visible,
    truncated: visible.length < lines.length || visible.some((line, index) => line.content !== lines[index]?.content),
  };
}

function parseReadFileRequest(input: Record<string, unknown>): ReadFileRequest {
  const path = typeof input['path'] === 'string' ? input['path'] : '';
  const offset = typeof input['offset'] === 'number' && Number.isFinite(input['offset'])
    ? input['offset']
    : null;
  const limit = typeof input['limit'] === 'number' && Number.isFinite(input['limit'])
    ? input['limit']
    : null;
  return { path, offset, limit };
}

// eslint-disable-next-line react-refresh/only-export-components -- pure parser test seam
export const __readFileResultTestables = {
  parseOutput: (text: string) => parseReadFileOutput(text),
};

function parseReadFileOutput(text: string): ReadFileParseResult {
  const trimmed = text.trim();
  if (trimmed === '') {
    return { lines: [], notes: [], malformed: false };
  }

  const lines: ReadFileLine[] = [];
  const notes: string[] = [];
  let malformed = false;

  for (const rawLine of text.split('\n')) {
    if (!rawLine.trim()) continue;
    const match = /^\s*(\d+)\t([\s\S]*)$/.exec(rawLine);
    if (match && match[1] !== undefined) {
      lines.push({ lineNumber: parseInt(match[1], 10), content: match[2] ?? '' });
      continue;
    }
    notes.push(rawLine);
    malformed = true;
  }

  return { lines, notes, malformed };
}

function formatReadFileRange(request: ReadFileRequest, parsed: ReadFileParseResult): string {
  const firstLine = parsed.lines[0]?.lineNumber;
  const lastLine = parsed.lines.at(-1)?.lineNumber;

  if (firstLine !== undefined && lastLine !== undefined) {
    return firstLine === lastLine ? `line ${firstLine}` : `lines ${firstLine}-${lastLine}`;
  }

  if (request.offset !== null && request.limit !== null) {
    const end = request.offset + request.limit - 1;
    return request.limit === 1 ? `line ${request.offset}` : `lines ${request.offset}-${end}`;
  }

  if (request.offset !== null) {
    return `from line ${request.offset}`;
  }

  return 'from start of file';
}

export function ReadFileResultView({
  input,
  rawText,
  metadata,
  onOpenFile,
  toolUseId,
  activeHighlight = null,
  showPath = true,
}: {
  input: Record<string, unknown>;
  rawText: string;
  metadata?: ReadFileDisplayData | null;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  toolUseId?: string | undefined;
  activeHighlight?: AgentTextHighlight | null;
  showPath?: boolean;
}) {
  const request = useMemo(() => parseReadFileRequest(input), [input]);
  const parsed = useMemo(() => parseReadFileOutput(rawText), [rawText]);
  const projection = useMemo(
    () => buildReadFileOutputProjection(rawText, input, toolUseId ? { toolUseId } : {}),
    [rawText, input, toolUseId],
  );
  const preview = useMemo(() => boundedReadFileLines(parsed.lines), [parsed.lines]);
  const [showAllReturnedLines, setShowAllReturnedLines] = useState(false);
  if (!metadata) {
    return (
      <ReadFileProjectionView
        projection={projection}
        onOpenFile={onOpenFile}
        activeHighlight={activeHighlight}
        showPath={showPath}
      />
    );
  }
  const fullFileViewerAvailable = Boolean(onOpenFile && metadata.viewer_available);
  const hasMore = preview.truncated;
  const canExpandReturnedOutput = hasMore;
  const activeLineNumber = activeHighlight
    ? projection.fragments.find((fragment) => fragment.fragmentId === activeHighlight.fragmentId)?.revealTarget.lineNumber
    : undefined;
  const activeLineNeedsReveal = activeLineNumber !== undefined
    && !preview.lines.some((line) => line.lineNumber === activeLineNumber);
  const visibleLines = showAllReturnedLines || activeLineNeedsReveal ? parsed.lines : preview.lines;
  const firstVisibleLine = metadata.returned_start_line ?? parsed.lines[0]?.lineNumber ?? request.offset ?? 0;
  const lastVisibleLine = metadata.returned_end_line ?? parsed.lines.at(-1)?.lineNumber ?? firstVisibleLine;
  const pathFragment = projection.fragments.find((fragment) => fragment.kind === 'path') ?? null;
  const pathHighlight = activeHighlight?.fragmentId === pathFragment?.fragmentId ? activeHighlight : null;
  const lineFragments = projection.fragments.filter((fragment) => fragment.kind === 'line');
  const noteFragments = projection.fragments.filter((fragment) => fragment.kind === 'note');

  if (metadata.total_line_count === 0) {
    return (
      <div className="read-file-result read-file-result-fallback" data-read-file-state="empty">
        <div className="read-file-result-meta">
          <span className="read-file-result-path" data-fragment-id={pathFragment?.fragmentId}>
            {pathHighlight && pathFragment
              ? renderHighlightedText(pathFragment.semanticText, pathHighlight.start, pathHighlight.end)
              : request.path || '(unknown path)'}
          </span>
          <span className="read-file-result-summary">No file content returned</span>
        </div>
        <div className="read-file-result-empty">(empty file)</div>
      </div>
    );
  }

  if (metadata.returned_line_count === 0) {
    return (
      <div className="read-file-result read-file-result-fallback" data-read-file-state="empty-range">
        <div className="read-file-result-meta">
          <span className="read-file-result-path" data-fragment-id={pathFragment?.fragmentId}>
            {pathHighlight && pathFragment
              ? renderHighlightedText(pathFragment.semanticText, pathHighlight.start, pathHighlight.end)
              : request.path || '(unknown path)'}
          </span>
          <span className="read-file-result-summary">No lines returned for the requested range</span>
        </div>
        <div className="read-file-result-empty">The file contains {metadata.total_line_count} lines.</div>
      </div>
    );
  }

  const rangeLabel = formatReadFileRange(request, parsed);

  return (
    <div className="read-file-result" data-read-file-state={parsed.malformed ? 'mixed' : 'structured'}>
      <div className="read-file-result-meta">
        <div className="read-file-result-meta-main">
          <span className="read-file-result-path" data-fragment-id={pathFragment?.fragmentId}>
            {pathHighlight && pathFragment
              ? renderHighlightedText(pathFragment.semanticText, pathHighlight.start, pathHighlight.end)
              : request.path || '(unknown path)'}
          </span>
          <span className="read-file-result-summary">
            {metadata.returned_line_count} line{metadata.returned_line_count === 1 ? '' : 's'} • {rangeLabel}
          </span>
          <span className="read-file-result-summary">of {metadata.total_line_count} total</span>
          <span className="read-file-result-summary">requested {metadata.requested_limit}</span>
        </div>
        <div className="read-file-result-actions">
          {fullFileViewerAvailable && (
            <button
              type="button"
              className="read-file-result-open"
              onClick={() => onOpenFile?.(request.path, new Set(), firstVisibleLine, lastVisibleLine)}
              title="View the complete current file focused on this range"
            >
              View full file
            </button>
          )}
          {canExpandReturnedOutput && (
            <button
              type="button"
              className="read-file-result-open"
              onClick={() => setShowAllReturnedLines((visible) => !visible)}
              aria-expanded={showAllReturnedLines}
            >
              {showAllReturnedLines ? 'Show preview' : 'Show all returned lines'}
            </button>
          )}
          <CopyButton text={rawText} title="Copy all returned lines" />
        </div>
      </div>
      <div className="read-file-result-preview" role="table" aria-label="read_file preview">
        {visibleLines.map((line) => {
          const fragment = lineFragments.find((candidate) => candidate.display.lineNumber === line.lineNumber);
          const highlight = activeHighlight?.fragmentId === fragment?.fragmentId ? activeHighlight : null;
          const lineNumberText = String(line.lineNumber);
          return (
            <div key={line.lineNumber} className="read-file-result-line" role="row" data-fragment-id={fragment?.fragmentId}>
              <span className="read-file-result-lineno" role="cell">
                {keywordFieldHighlight(highlight, 0, lineNumberText)}
              </span>
              <span className="read-file-result-content" role="cell">
                {keywordFieldHighlight(highlight, lineNumberText.length + 1, line.content || ' ')}
              </span>
            </div>
          );
        })}
      </div>
      {noteFragments.map((fragment) => {
        const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
        return (
          <div key={fragment.fragmentId} className="read-file-result-note" data-fragment-id={fragment.fragmentId}>
            {highlight
              ? renderHighlightedText(fragment.semanticText, highlight.start, highlight.end)
              : fragment.display.note}
          </div>
        );
      })}
      {(hasMore || metadata.remaining_line_count > 0) && (
        <div className="read-file-result-more">
          {hasMore && !showAllReturnedLines && (parsed.lines.length > visibleLines.length
            ? `${parsed.lines.length - visibleLines.length} more returned lines`
            : 'returned line truncated for preview')}
          {hasMore && !showAllReturnedLines && metadata.remaining_line_count > 0 && ' · '}
          {metadata.remaining_line_count > 0 && `${metadata.remaining_line_count} file lines not returned`}
          {fullFileViewerAvailable && ' · view the complete current file for full context'}
        </div>
      )}
    </div>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function parseSearchOutput(text: string) {
  return buildSearchOutputProjection(text);
}

function ReadFileProjectionView({
  projection,
  onOpenFile,
  activeHighlight,
  showPath,
}: {
  projection: ReturnType<typeof buildReadFileOutputProjection>;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  activeHighlight: AgentTextHighlight | null;
  showPath: boolean;
}) {
  const pathFragment = projection.fragments.find((fragment) => fragment.kind === 'path') ?? null;
  const pathHighlight = activeHighlight?.fragmentId === pathFragment?.fragmentId ? activeHighlight : null;
  const lineFragments = projection.fragments.filter((fragment) => fragment.kind === 'line');
  const noteFragments = projection.fragments.filter((fragment) => fragment.kind === 'note');

  return (
    <div className="read-file-results">
      {showPath && pathFragment && (
        onOpenFile ? (
          <button
            type="button"
            className="search-results-filepath"
            data-fragment-id={pathFragment.fragmentId}
            onClick={() => onOpenFile(pathFragment.display.path, new Set(), 0)}
            title="Open file"
          >
            {pathHighlight
              ? renderHighlightedText(pathFragment.semanticText, pathHighlight.start, pathHighlight.end)
              : pathFragment.semanticText}
          </button>
        ) : (
          <div
            className="search-results-filepath search-results-filepath-static"
            data-fragment-id={pathFragment.fragmentId}
          >
            {pathHighlight
              ? renderHighlightedText(pathFragment.semanticText, pathHighlight.start, pathHighlight.end)
              : pathFragment.semanticText}
          </div>
        )
      )}
      <div className="search-results-hits">
        {lineFragments.map((fragment) => {
          const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
          const lineNumber = fragment.display.lineNumber;
          const lineContent = fragment.display.content || ' ';
          const lineNumberText = String(lineNumber);
          const contentStart = lineNumberText.length + 1;
          const body = (
            <>
              <span className="search-result-lineno">
                {keywordFieldHighlight(highlight, 0, lineNumberText)}
              </span>
              <span className="search-result-content">
                {keywordFieldHighlight(highlight, contentStart, lineContent)}
              </span>
            </>
          );
          return onOpenFile && pathFragment ? (
            <button
              key={fragment.fragmentId}
              type="button"
              className="search-result-line search-result-line-clickable"
              data-fragment-id={fragment.fragmentId}
              onClick={() => onOpenFile(pathFragment.display.path, new Set([lineNumber]), lineNumber)}
            >
              {body}
            </button>
          ) : (
            <div key={fragment.fragmentId} className="search-result-line" data-fragment-id={fragment.fragmentId}>
              {body}
            </div>
          );
        })}
      </div>
      {noteFragments.map((fragment) => {
        const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
        return (
          <div key={fragment.fragmentId} className="read-file-result-note" data-fragment-id={fragment.fragmentId}>
            {highlight
              ? renderHighlightedText(fragment.semanticText, highlight.start, highlight.end)
              : fragment.display.note}
          </div>
        );
      })}
    </div>
  );
}

export function TerminalToolResultHighlight({
  semanticText,
  fragmentId,
  activeHighlight,
}: {
  semanticText: string;
  fragmentId: string;
  activeHighlight: AgentTextHighlight;
}) {
  const maxLength = 5_000;
  const windowStart = semanticText.length > maxLength
    ? Math.max(0, Math.min(activeHighlight.start - Math.floor(maxLength / 2), semanticText.length - maxLength))
    : 0;
  const visibleText = semanticText.slice(windowStart, windowStart + maxLength);
  const visibleHighlight = {
    ...activeHighlight,
    start: activeHighlight.start - windowStart,
    end: activeHighlight.end - windowStart,
  };
  return (
    <pre className="tool-block-output-content" data-fragment-id={fragmentId}>
      {windowStart > 0 ? '…\n' : ''}
      {renderHighlightedText(visibleText, visibleHighlight.start, visibleHighlight.end)}
      {windowStart + visibleText.length < semanticText.length ? '\n…' : ''}
    </pre>
  );
}

export function PatchResultView({
  diff,
  toolUseId,
  activeHighlight = null,
}: {
  diff: string;
  toolUseId?: string | undefined;
  activeHighlight?: AgentTextHighlight | null;
}) {
  const projection = useMemo(
    () => buildPatchOutputProjection(diff, toolUseId ? { toolUseId } : {}),
    [diff, toolUseId],
  );
  const fragment = projection.fragments[0];
  const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
  const maxLength = 5_000;
  const windowStart = highlight && fragment.display.diff.length > maxLength
    ? Math.max(0, Math.min(highlight.start - Math.floor(maxLength / 2), fragment.display.diff.length - maxLength))
    : 0;
  const visibleDiff = highlight
    ? fragment.display.diff.slice(windowStart, windowStart + maxLength)
    : fragment.display.diff;
  return (
    <div className="tool-block-output-content" data-fragment-id={fragment.fragmentId}>
      {highlight ? (
        <>
          {windowStart > 0 ? '…\n' : ''}
          {renderHighlightedText(visibleDiff, highlight.start - windowStart, highlight.end - windowStart)}
          {windowStart + visibleDiff.length < fragment.display.diff.length ? '\n…' : ''}
        </>
      ) : fragment.display.diff}
    </div>
  );
}

function renderSearchResultPath(
  path: string,
  fragmentId: string,
  activeHighlight: AgentTextHighlight | null,
): React.ReactNode {
  if (activeHighlight?.fragmentId !== fragmentId) return path;
  return keywordFieldHighlight(activeHighlight, 0, path);
}

export function SearchResultsView({
  rawText,
  onOpenFile,
  toolUseId,
  activeHighlight = null,
}: {
  rawText: string;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number, focusEndLine?: number) => void) | undefined;
  toolUseId?: string | undefined;
  activeHighlight?: AgentTextHighlight | null;
}) {
  const parsed = useMemo(() => buildSearchOutputProjection(rawText, toolUseId ? { toolUseId } : {}), [rawText, toolUseId]);

  if (parsed.noMatches) {
    return (
      <div className="search-results">
        <div className="search-results-empty" data-fragment-id={parsed.fragments[0]?.fragmentId}>
          {activeHighlight?.fragmentId === parsed.fragments[0]?.fragmentId && parsed.fragments[0]
            ? renderHighlightedText(parsed.fragments[0].semanticText, activeHighlight!.start, activeHighlight!.end)
            : 'No matches found.'}
        </div>
      </div>
    );
  }

  if (parsed.rawFallback) {
    const fragment = parsed.fragments[0] ?? null;
    const highlight = activeHighlight?.fragmentId === fragment?.fragmentId ? activeHighlight : null;
    return <pre className="search-results-fallback" data-fragment-id={fragment?.fragmentId}>{highlight && fragment ? renderHighlightedText(fragment.semanticText, highlight.start, highlight.end) : rawText}</pre>;
  }

  return (
    <div className="search-results">
      {parsed.hits.length > 0 && (
        <div className="search-results-header">
          <span className="search-results-count">
            {parsed.hits.length} match{parsed.hits.length === 1 ? '' : 'es'} in {parsed.groups.length} file
            {parsed.groups.length === 1 ? '' : 's'}
          </span>
        </div>
      )}
      <div className="search-results-list">
        {parsed.groups.map((group) => (
          <div key={group.path} className="search-results-file">
            {onOpenFile ? (
              <button
                type="button"
                className="search-results-filepath"
                onClick={() =>
                  onOpenFile(
                    group.path,
                    new Set([group.hits[0]!.lineNumber]),
                    group.hits[0]!.lineNumber,
                  )
                }
                title="Open file"
                data-fragment-id={group.fragment.fragmentId}
              >
                {renderSearchResultPath(group.path, group.fragment.fragmentId, activeHighlight)}
                <span className="search-results-filehit-count">
                  {group.hits.length} hit{group.hits.length === 1 ? '' : 's'}
                </span>
              </button>
            ) : (
              <span
                className="search-results-filepath search-results-filepath-static"
                data-fragment-id={group.fragment.fragmentId}
              >
                {renderSearchResultPath(group.path, group.fragment.fragmentId, activeHighlight)}
                <span className="search-results-filehit-count">
                  {group.hits.length} hit{group.hits.length === 1 ? '' : 's'}
                </span>
              </span>
            )}
            <div className="search-results-hits">
              {group.hits.map((hit) => {
                const highlight = activeHighlight?.fragmentId === hit.fragment.fragmentId ? activeHighlight : null;
                const lineText = String(hit.lineNumber);
                const contentStart = lineText.length + 2;
                const lineNumberContent = (
                  <>
                    <span className="search-result-lineno">
                      {keywordFieldHighlight(highlight, 0, lineText)}
                    </span>
                    <span className="search-result-content">
                      {keywordFieldHighlight(highlight, contentStart, hit.content || ' ')}
                    </span>
                  </>
                );
                return onOpenFile ? (
                  <button
                    key={hit.fragment.fragmentId}
                    type="button"
                    className="search-result-line search-result-line-clickable"
                    data-fragment-id={hit.fragment.fragmentId}
                    onClick={() => onOpenFile(group.path, new Set([hit.lineNumber]), hit.lineNumber)}
                  >
                    {lineNumberContent}
                  </button>
                ) : (
                  <div key={hit.fragment.fragmentId} className="search-result-line" data-fragment-id={hit.fragment.fragmentId}>
                    {lineNumberContent}
                  </div>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      {parsed.notes.length > 0 && (
        <div className="search-results-notes">
          {parsed.notes.map(({ text, fragment }) => {
            const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
            return (
              <div key={fragment.fragmentId} className="search-results-note" data-fragment-id={fragment.fragmentId}>
                {highlight ? renderHighlightedText(fragment.semanticText, highlight.start, highlight.end) : text}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function parseKeywordSearchOutput(text: string) {
  const projection = buildKeywordSearchOutputProjection(text);
  return { ...projection, notes: projection.notes.map((note) => note.text) };
}

function keywordFieldHighlight(
  highlight: AgentTextHighlight | null,
  fieldStart: number,
  fieldText: string,
): React.ReactNode {
  if (!highlight) return fieldText;
  const start = Math.max(0, highlight.start - fieldStart);
  const end = Math.min(fieldText.length, highlight.end - fieldStart);
  return end > start ? renderHighlightedText(fieldText, start, end) : fieldText;
}

export function KeywordSearchView({
  rawText,
  onOpenFile,
  toolUseId,
  activeHighlight = null,
}: {
  rawText: string;
  onOpenFile: ((filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void) | undefined;
  toolUseId?: string | undefined;
  activeHighlight?: AgentTextHighlight | null;
}) {
  const parsed = useMemo(
    () => buildKeywordSearchOutputProjection(rawText, toolUseId ? { toolUseId } : {}),
    [rawText, toolUseId],
  );

  const notesEl = parsed.notes.length > 0 ? (
    <div className="search-results-notes">
      {parsed.notes.map(({ text, fragment }) => {
        const highlight = activeHighlight?.fragmentId === fragment.fragmentId ? activeHighlight : null;
        return (
          <div key={fragment.fragmentId} className="search-results-note" data-fragment-id={fragment.fragmentId}>
            {highlight ? renderHighlightedText(fragment.semanticText, highlight.start, highlight.end) : text}
          </div>
        );
      })}
    </div>
  ) : null;

  if (parsed.empty) {
    return (
      <div className="keyword-search-results">
        <div className="search-results-empty" data-fragment-id={parsed.fragments[0]?.fragmentId}>
          {activeHighlight?.fragmentId === parsed.fragments[0]?.fragmentId && parsed.fragments[0]
            ? renderHighlightedText(parsed.fragments[0].semanticText, activeHighlight!.start, activeHighlight!.end)
            : 'No relevant files found.'}
        </div>
        {notesEl}
      </div>
    );
  }

  if (parsed.rawFallback) {
    const titleFragment = parsed.fragments.find((fragment) => fragment.fragmentId === 'keyword-search-fallback-title') ?? null;
    const bodyFragment = parsed.fragments.find((fragment) => fragment.fragmentId === 'keyword-search-fallback-body') ?? null;
    const titleHighlight = activeHighlight?.fragmentId === titleFragment?.fragmentId ? activeHighlight : null;
    const bodyHighlight = activeHighlight?.fragmentId === bodyFragment?.fragmentId ? activeHighlight : null;
    return (
      <div className="keyword-search-results keyword-search-raw">
        <div className="keyword-search-fallback-note" data-fragment-id={titleFragment?.fragmentId}>
          {titleHighlight && titleFragment
            ? renderHighlightedText(titleFragment.semanticText, titleHighlight.start, titleHighlight.end)
            : 'Raw ripgrep results — LLM filter unavailable'}
        </div>
        <pre className="keyword-search-raw-text" data-fragment-id={bodyFragment?.fragmentId}>
          {bodyHighlight && bodyFragment
            ? renderHighlightedText(bodyFragment.semanticText, bodyHighlight.start, bodyHighlight.end)
            : parsed.fallbackText}
        </pre>
        {notesEl}
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
      {notesEl}
      <div className="keyword-search-list">
        {parsed.hits.map((hit) => {
          const highlight = activeHighlight?.fragmentId === hit.fragment.fragmentId ? activeHighlight : null;
          return (
            <div key={hit.fragment.fragmentId} className="keyword-search-hit" data-fragment-id={hit.fragment.fragmentId}>
              {onOpenFile ? (
                <button
                  type="button"
                  className="keyword-search-filepath"
                  onClick={() => onOpenFile(hit.path, new Set(), 0)}
                >
                  {keywordFieldHighlight(highlight, 0, hit.path)}
                </button>
              ) : (
                <span className="keyword-search-filepath keyword-search-filepath-static">
                  {keywordFieldHighlight(highlight, 0, hit.path)}
                </span>
              )}
              <div className="keyword-search-explanation">
                {keywordFieldHighlight(highlight, hit.path.length + 2, hit.explanation)}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export const ToolUseBlock = memo(ToolUseBlockImpl);

function ToolUseBlockImpl({ block, result, onOpenFile, onOpenCommissionReview, requestSequenceId, workScopeKey, knownResultIds, toolStartedAtMs, showMissingResult, liveBashProgress, revealRequest = null, activeHighlight = null, onRevealHandled }: ToolUseBlockProps) {
  const name = block.name || 'tool';
  const input = useMemo(() => block.input || {}, [block.input]);
  const toolId = block.id || '';

  // Format the input display based on tool type
  // For bash, use server-provided display field (has cd prefix stripped)
  const { display: inputDisplay, isMultiline: inputIsMultiline } = formatToolInput(
    name,
    input as Record<string, unknown>,
    block.display
  );

  const toolCardState = getToolCardState({ toolId, result, toolStartedAtMs });
  const resultContent =
    toolCardState.kind === 'completed' || toolCardState.kind === 'failed'
      ? toolCardState.resultContent
      : null;
  const durationMs =
    toolCardState.kind === 'completed' || toolCardState.kind === 'failed'
      ? toolCardState.durationMs
      : undefined;

  const runningStartedAtMs = toolCardState.kind === 'running' ? toolCardState.toolStartedAtMs : undefined;

  // REQ-WPV-002: live elapsed counter while the tool is in flight
  // (block exists, no result yet). Source is the server-authoritative
  // `tool_starts[block.id]` stamped by `dispatch_tool_execution`, so
  // the counter survives reconnect / reload / multi-tab. Cleared the
  // instant the result lands (the static `durationMs` from the tool
  // result takes over).
  const [inflightElapsedSeconds, setInflightElapsedSeconds] = useState(0);
  useEffect(() => {
    if (toolCardState.kind !== 'running' || runningStartedAtMs === undefined) {
      setInflightElapsedSeconds(0);
      return;
    }
    const startedAtMs = runningStartedAtMs;
    const compute = () =>
      setInflightElapsedSeconds(Math.max(0, Math.floor((Date.now() - startedAtMs) / 1000)));
    compute();
    const interval = window.setInterval(compute, 1000);
    return () => window.clearInterval(interval);
  }, [toolCardState.kind, runningStartedAtMs]);

  useEffect(() => {
    if (toolCardState.kind === 'missing_result') {
      logMissingToolResult({ toolId, name, knownResultIds, toolStartedAtMs });
    }
  }, [knownResultIds, name, toolCardState.kind, toolId, toolStartedAtMs]);

  const rawResultText = getToolResultTextFromContent(resultContent);
  const isError = toolCardState.kind === 'failed';

  // For bash/tmux, the tool result is a structured JSON envelope (REQ-BASH-002 /
  // REQ-TMUX-012). Decode it once so the renderer below can branch on
  // status / running state / label rather than show the raw JSON.
  const bashResponse = useMemo(() => (name === 'bash' ? tryParseJson(rawResultText) : null), [name, rawResultText]);
  const tmuxResponse = useMemo(() => (name === 'tmux' ? tryParseJson(rawResultText) : null), [name, rawResultText]);
  const profileAction = name === 'browser_profile' && typeof input['action'] === 'string' ? input['action'] : null;
  const terminalResultFamily: TerminalToolResultFamily = name === 'bash' || name === 'tmux'
    ? name
    : isError
      ? profileAction !== null && STRUCTURED_PROFILE_ACTIONS.has(profileAction)
        ? 'browser-profile'
        : 'opaque'
      : name === 'browser_recent_console_logs'
        ? 'console-logs'
        : profileAction !== null && STRUCTURED_PROFILE_ACTIONS.has(profileAction)
          ? 'browser-profile'
          : 'opaque';
  // For patch tool, use the diff from display_data instead of the generic success message
  const patchDiff = name === 'patch' ? (result?.display_data as { diff?: string })?.diff : undefined;
  const resultText = patchDiff || rawResultText;
  const terminalProjection = useMemo(
    () => buildTerminalToolResultProjection(terminalResultFamily, resultText, result?.display_data, { toolUseId: toolId }),
    [result?.display_data, resultText, terminalResultFamily, toolId],
  );
  const resultLength = resultText.length;
  const keywordSearchProjection = useMemo(
    () => (name === 'keyword_search' ? buildKeywordSearchOutputProjection(resultText, { toolUseId: toolId }) : null),
    [name, resultText, toolId],
  );
  const searchProjection = useMemo(
    () => (name === 'search' ? buildSearchOutputProjection(resultText, { toolUseId: toolId }) : null),
    [name, resultText, toolId],
  );
  const firstKeywordTarget = keywordSearchProjection?.fragments[0]?.revealTarget;
  const firstSearchTarget = searchProjection?.fragments[0]?.revealTarget;
  const toolRevealKey = name === 'keyword_search'
    && firstKeywordTarget?.kind === 'tool-result-keyword-search'
    ? firstKeywordTarget.key
    : name === 'search' && firstSearchTarget?.kind === 'tool-result-search'
      ? firstSearchTarget.key
      : null;
  const readFileProjection = useMemo(
    () => (name === 'read_file' ? buildReadFileOutputProjection(resultText, input as Record<string, unknown>, { toolUseId: toolId }) : null),
    [input, name, resultText, toolId],
  );
  const patchProjection = useMemo(
    () => (name === 'patch' ? buildPatchOutputProjection(resultText, { toolUseId: toolId }) : null),
    [name, resultText, toolId],
  );
  const subAgentFragments = useMemo(
    () => buildSubAgentCardFragments(result?.display_data, toolId),
    [result?.display_data, toolId],
  );
  const toolActiveHighlight = activeHighlight?.owner === 'tool-result'
    && activeHighlight.toolUseId === toolId
    && activeHighlight.fragmentId
    && ((name === 'keyword_search' && keywordSearchProjection?.fragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId))
      || (name === 'search' && searchProjection?.fragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId))
      || (name === 'read_file' && readFileProjection?.fragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId))
      || (name === 'patch' && patchProjection?.fragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId))
      || terminalProjection.fragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId)
      || subAgentFragments.some((fragment) => fragment.fragmentId === activeHighlight.fragmentId)
      || (name === 'browser_profile' && activeHighlight.fragmentId === 'browser-profile-visible')
      || (name === 'skill' && activeHighlight.fragmentId === 'skill-result-visible')
      || (name === 'commission_review' && activeHighlight.fragmentId.startsWith('commission-review-')))
    ? activeHighlight
    : null;
  const inputActiveHighlight = activeHighlight?.owner === 'tool-input'
    && activeHighlight.toolUseId === toolId
    && activeHighlight.fragmentId === 'tool-use-input'
    ? activeHighlight
    : null;

  
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

  const readFileMetadata = name === 'read_file'
    ? parseReadFileDisplayData(result?.display_data)
    : null;

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

  useEffect(() => {
    if (!revealRequest) return;
    if (revealRequest.revealTarget.kind === 'tool-use-input') {
      if (revealRequest.revealTarget.toolUseId === toolId) onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind === 'tool-result-browser-profile'
      || revealRequest.revealTarget.kind === 'tool-result-commission-review') {
      if (revealRequest.revealTarget.toolUseId === toolId) onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind === 'tool-result-read-file') {
      if (revealRequest.revealTarget.toolUseId !== toolId) return;
      onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind === 'tool-result-patch') {
      if (revealRequest.revealTarget.toolUseId !== toolId) return;
      if (!outputExpanded) {
        setOutputExpanded(true);
        return;
      }
      onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind === 'tool-result-terminal') {
      if (revealRequest.revealTarget.toolUseId !== toolId) return;
      if (!outputExpanded) {
        setOutputExpanded(true);
        return;
      }
      onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind === 'subagent-card') {
      if (revealRequest.revealTarget.toolUseId !== toolId) return;
      onRevealHandled?.(revealRequest);
      return;
    }
    if (revealRequest.revealTarget.kind !== 'tool-result-keyword-search' && revealRequest.revealTarget.kind !== 'tool-result-search') return;
    if (revealRequest.revealTarget.key !== toolRevealKey) return;
    onRevealHandled?.(revealRequest);
  }, [onRevealHandled, outputExpanded, revealRequest, toolId, toolRevealKey]);

  // For display, truncate very long outputs even when expanded
  const displayResult = useMemo(() => {
    const maxDisplayLen = 5000;
    return resultText.length > maxDisplayLen
      ? resultText.slice(0, maxDisplayLen) + `\n... (${resultText.length - maxDisplayLen} more chars)`
      : resultText;
  }, [resultText]);

  const cappedResultText = useMemo(() => resultText.slice(0, 5000), [resultText]);

  // Preview for collapsed state: show first 3 lines faded. Split once, derive both.
  const lines = useMemo(() => resultText.split('\n'), [resultText]);
  const previewLines = lines.slice(0, 3);
  const lineCount = lines.length;
  const hasMoreLines = lineCount > 3;

  const hasOutput = resultContent !== null;
  const isShortOutput = resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;
  const isSubAgentResult = !!(result?.display_data && isSubAgentSummaryData(result.display_data));

  // Decoupled task fork proposal (REQ-PROJ-034): a writing-mode `propose_task`
  // records its proposal id on the synthetic success tool-result's
  // `display_data.fork_proposal_id`. That id anchors the inline Review
  // affordance (which cross-references the proposal's status from the
  // ForkProposals store and withdraws once resolved).
  const forkProposalId: string | undefined = (() => {
    const dd = result?.display_data as Record<string, unknown> | undefined;
    const v = dd?.['fork_proposal_id'];
    return typeof v === 'string' && v.length > 0 ? v : undefined;
  })();

  // Get the raw input for copying (not the formatted display)
  if (name === 'skill') {
    return (
      <SkillToolBlock
        input={input as Record<string, unknown>}
        resultText={resultText}
        result={result}
        isError={isError}
        durationMs={durationMs}
        toolStartedAtMs={toolStartedAtMs}
        inflightElapsedSeconds={inflightElapsedSeconds}
        onOpenFile={onOpenFile}
        toolId={toolId}
        activeHighlight={toolActiveHighlight}
        inputActiveHighlight={inputActiveHighlight}
      />
    );
  }

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

  const commissionReviewDisplayData = name === 'commission_review'
    ? parseCommissionReviewResult(result?.display_data, resultText)
    : null;
  const commissionReviewInput = name === 'commission_review'
    ? parseCommissionReviewInput(input as Record<string, unknown>)
    : null;

  return (
    <div className="tool-block" data-tool-id={toolId}>
      {/* Tool header with name */}
      <div className="tool-block-header">
        <span className="tool-block-name">{name}</span>
        {renderToolCardState(toolCardState, inflightElapsedSeconds)}
      </div>

      {/* Tool input - always visible */}
      {commissionReviewInput ? (
        <CommissionReviewInputView input={commissionReviewInput} activeHighlight={inputActiveHighlight} />
      ) : (
        <div
          className={`tool-block-input ${inputIsMultiline ? 'multiline' : ''}`}
          data-fragment-id="tool-use-input"
        >
          {inputActiveHighlight
            ? renderHighlightedText(inputDisplay, inputActiveHighlight.start, inputActiveHighlight.end)
            : inputDisplay}
          <CopyButton text={rawInput} title={bashCopyTitle} />
        </div>
      )}

      {/* Tool output - collapsible for long outputs; suppressed when structured summary is shown */}
      {name === 'bash' && !result && liveBashProgress && (
        <div className="tool-block-output bash-live-output">
          <BashOutputView
            lines={liveBashProgress.lines.map((line) => line.text)}
            partial={liveBashProgress.partial ?? null}
            truncatedBefore={liveBashProgress.truncated_before}
            bounded
          />
        </div>
      )}
      {showMissingResult && !hasOutput && renderMissingToolResultBody(toolCardState)}
      {hasOutput && !isSubAgentResult && (
        <div className={`tool-block-output ${isError ? 'error' : ''} ${outputExpanded ? 'expanded' : ''}`}>
          {toolActiveHighlight?.fragmentId === terminalProjection.fragments[0].fragmentId ? (
            <TerminalToolResultHighlight
              semanticText={terminalProjection.fullText}
              fragmentId={terminalProjection.fragments[0].fragmentId}
              activeHighlight={toolActiveHighlight}
            />
          ) : imageResult ? (
            // Image result: render as image
            <div className="tool-block-image-output">
              <img
                src={`data:${imageResult.media_type};base64,${imageResult.data}`}
                alt="Tool result"
                className="message-image"
              />
            </div>
          ) : bashResponse ? (
            <BashResponseView response={bashResponse} workScopeKey={workScopeKey} />
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
              activeHighlight={toolActiveHighlight}
            />
          ) : name === 'browser_recent_console_logs' && !isError ? (
            <BrowserConsoleLogsView rawText={resultText} />
          ) : name === 'search' && !isError ? (
            <SearchResultsView rawText={resultText} onOpenFile={onOpenFile} toolUseId={toolId} activeHighlight={toolActiveHighlight} />
          ) : name === 'keyword_search' && !isError ? (
            <KeywordSearchView rawText={resultText} onOpenFile={onOpenFile} toolUseId={toolId} activeHighlight={toolActiveHighlight} />
          ) : name === 'read_file' && !isError ? (
            <>
              <ReadFileResultView
                rawText={readFileMetadata || (toolActiveHighlight && toolActiveHighlight.fragmentId !== 'read-file-path')
                  ? resultText
                  : cappedResultText}
                input={input as Record<string, unknown>}
                onOpenFile={onOpenFile}
                toolUseId={toolId}
                metadata={readFileMetadata}
                activeHighlight={toolActiveHighlight}
                showPath={toolActiveHighlight?.fragmentId === 'read-file-path'}
              />
              {!readFileMetadata && !toolActiveHighlight && resultText.length > 5000 && (
                <div className="tool-output-truncation">... ({resultText.length - 5000} more chars)</div>
              )}
            </>
          ) : name === 'patch' && !isError && outputExpanded ? (
            <PatchResultView
              diff={toolActiveHighlight ? resultText : displayResult}
              toolUseId={toolId}
              activeHighlight={toolActiveHighlight}
            />
          ) : commissionReviewDisplayData ? (
            <CommissionReviewSummaryCard
              data={commissionReviewDisplayData}
              activeHighlight={toolActiveHighlight}
              formatDuration={formatToolDuration}
              requestSequenceId={requestSequenceId}
              onOpenFullReview={onOpenCommissionReview}
            />
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
        <SubAgentSummary
          results={result.display_data.results}
          revealRequest={revealRequest?.revealTarget.kind === 'subagent-card' ? revealRequest : null}
          activeHighlight={toolActiveHighlight}
        />
      )}

      {/* Fork proposal Review affordance (REQ-PROJ-034 / 037) */}
      {forkProposalId && <ForkProposalAffordance proposalId={forkProposalId} />}
    </div>
  );
}

// =====================================================================// Sub-Agent Summary (persistent view after completion)
// =====================================================================
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

function ChildToolActivity({ block, result, liveProgress }: { block: ContentBlock; result: Message | undefined; liveProgress?: import('../generated/sse').BashToolProgress | undefined }) {
  const name = block.name || 'tool';
  const input = (block.input || {}) as Record<string, unknown>;
  const output = getToolResultText(result);
  const firstOutputLine = output.split('\n').find((line) => line.trim())?.trim() ?? '';
  const liveOutput = liveProgress
    ? [...liveProgress.lines.map((line) => line.text), ...(liveProgress.partial ? [liveProgress.partial] : [])].slice(-2).join(' · ')
    : '';
  const outputPreview = firstOutputLine ? truncate(firstOutputLine, 140) : liveOutput ? truncate(liveOutput, 140) : result ? '(empty)' : 'running…';
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

// Memoized so completed sub-agent steps are not re-parsed through ReactMarkdown
// on every streaming token of the *active* step. The transcript re-renders per
// token (the buffer grows), but a finished step's `message` object and its
// `toolResults` map are referentially stable across token-only atom updates, so
// a shallow prop compare bails. Mirrors the AgentTextBlock / StreamingBlock
// memoization for the same re-parse-on-unchanged-content problem.
const ChildAgentActivity = memo(function ChildAgentActivity({ message, toolResults, liveBashProgress, markdownComponents }: { message: Message; toolResults: Map<string, Message>; liveBashProgress: import('../conversation/atom').ConversationAtom['liveBashProgress']; markdownComponents: React.ComponentProps<typeof ReactMarkdown>['components'] }) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  return (
    <>
      {blocks.map((block, idx) => {
        if (block.type === 'text') {
          const text = block.text?.trim();
          if (!text) return null;
          return (
            <div key={`${message.message_id}-text-${idx}`} className="subagent-activity-event agent-text">
              <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={markdownComponents} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>{text.length > 900 ? `${text.slice(0, 900)}…` : text}</ReactMarkdown>
            </div>
          );
        }
        if (block.type === 'tool_use') {
          return (
            <ChildToolActivity
              key={block.id || `${message.message_id}-tool-${idx}`}
              block={block}
              result={toolResults.get(block.id || '')}
              liveProgress={liveBashProgress[block.id || '']?.progress}
            />
          );
        }
        return null;
      })}
    </>
  );
});

/**
 * Presentational read-only sub-agent transcript. Stream ownership lives with
 * the caller so the renderer is reused by both the inline peek
 * (`ChildConversationActivity`, which opens its own stream) and the always-
 * mounted docked viewer (`SubAgentViewerPanel`, which owns the stream so it can
 * derive live status from the same source it renders — see that component for
 * why card-local state can't be the source of truth under list virtualization).
 *
 * `running` drives only the "live" badge; `full` shows every agent step instead
 * of the latest dozen.
 */
export function SubAgentTranscript({ inline, running, full = false, finalResult }: { inline: InlineStreamState; running: boolean; full?: boolean; finalResult?: { text: string; statusClass: string } | undefined }) {
  const { atom } = inline;
  const messages = atom.messages;
  // Derived once per messages change, not per streaming token. `sse_token`
  // preserves `atom.messages` identity (only `streamingBuffer` grows), so a
  // stable `toolResults` map lets the memoized ChildAgentActivity rows bail
  // while the active step's buffer streams.
  const toolResults = useMemo(() => buildToolResults(messages), [messages]);
  const agentMessages = useMemo(
    () => messages.filter((m) => m.message_type === 'agent' || m.type === 'agent'),
    [messages],
  );
  const visibleAgentMessages = useMemo(
    () => (full ? agentMessages : agentMessages.slice(-12)),
    [full, agentMessages],
  );
  const hiddenCount = Math.max(0, agentMessages.length - visibleAgentMessages.length);
  const toolCount = useMemo(() => countToolUses(messages), [messages]);
  const rootDir = atom.conversation?.worktree_path ?? atom.conversation?.cwd ?? undefined;
  const markdownComponents = useMemo(
    () => (rootDir ? createConversationMarkdownComponents({ rootDir }) : CONVERSATION_MARKDOWN_COMPONENTS),
    [rootDir],
  );

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
        <ChildAgentActivity key={message.message_id} message={message} toolResults={toolResults} liveBashProgress={atom.liveBashProgress} markdownComponents={markdownComponents} />
      ))}
      {atom.streamingBuffer?.text && (
        <div className="subagent-activity-event agent-text streaming">
          <StreamingBlocks text={atom.streamingBuffer.text} rootDir={rootDir} />
        </div>
      )}
      {inline.type !== 'connecting' && inline.type !== 'error' && visibleAgentMessages.length === 0 && !atom.streamingBuffer?.text && (
        <div className="subagent-activity-placeholder">No sub-agent activity yet.</div>
      )}
      {finalResult?.text && (
        <div className={`subagent-final-result ${finalResult.statusClass}`}>
          <div className="subagent-final-result-label">final outcome</div>
          <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={markdownComponents} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>{finalResult.text}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}

/**
 * Inline peek at a sub-agent's activity inside the parent's `spawn_agents`
 * card. Owns its own read-only stream; `running` is the card's authoritative
 * status (the card is mounted whenever it's visible).
 */
function ChildConversationActivity({ agentId, expanded, running, finalResult }: { agentId: string; expanded: boolean; running: boolean; finalResult?: { text: string; statusClass: string } | undefined }) {
  const inline = useConversationInlineStream(agentId, expanded, running);
  if (!expanded) return null;
  return <SubAgentTranscript inline={inline} running={running} finalResult={finalResult} />;
}

function SubAgentActivityCard({
  agentId,
  task,
  outcome,
  revealRequest = null,
  activeHighlight = null,
}: {
  agentId: string;
  task: string;
  outcome: SubAgentResult['outcome'] | null;
  revealRequest?: AgentTextRevealRequest | null;
  activeHighlight?: AgentTextHighlight | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const status = statusKindFromOutcome(outcome);
  const statusClass = status.replace('_', '-');
  const running = status === 'running';
  const resultText = outcome ? getOutcomeText(outcome) : '';
  const fragmentId = `subagent-card:${agentId}`;
  const isActive = activeHighlight?.fragmentId === fragmentId;
  useEffect(() => {
    if (revealRequest?.revealTarget.kind !== 'subagent-card' || revealRequest.revealTarget.agentId !== agentId) return;
    setExpanded(true);
  }, [agentId, revealRequest]);
  const semanticText = [task, resultText].filter(Boolean).join('\n');

  return (
    <div className={`subagent-item activity ${statusClass}`} data-fragment-id={fragmentId}>
      <div className="subagent-item-header">
        {isActive && activeHighlight ? (
          <div className="subagent-result">
            {renderHighlightedText(semanticText, activeHighlight.start, activeHighlight.end)}
          </div>
        ) : null}
        <button
          type="button"
          className="subagent-expand-button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          <span className="subagent-icon"><SubAgentStatusIcon status={status} /></span>
          {!isActive && <span className="subagent-label" title={task}>{truncate(task, 72)}</span>}
          <span className="subagent-activity-count">activity</span>
          <span className={`subagent-status ${statusClass}`}>{getStatusLabel(status)}</span>
          <span className="subagent-expand-toggle">{expanded ? <ChevronUpIcon /> : <ChevronDownIcon />}</span>
        </button>
        <OpenConversationButton agentId={agentId} task={task} />
      </div>
      {!isActive && resultText && !expanded && (
        <div className="subagent-result preview">{truncate(resultText, 180)}</div>
      )}
      {!isActive && (
        <ChildConversationActivity
          agentId={agentId}
          expanded={expanded}
          running={running}
          finalResult={resultText ? { text: resultText, statusClass } : undefined}
        />
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
function SubAgentSummaryRow({ result, revealRequest, activeHighlight }: { result: SubAgentResult; revealRequest?: AgentTextRevealRequest | null; activeHighlight?: AgentTextHighlight | null }) {
  return (
    <SubAgentActivityCard
      agentId={result.agent_id}
      task={result.task}
      outcome={result.outcome}
      {...(revealRequest !== undefined ? { revealRequest } : {})}
      {...(activeHighlight !== undefined ? { activeHighlight } : {})}
    />
  );
}

/** Persistent summary of completed subagents (shown in spawn_agents tool result) */
function SubAgentSummary({ results, revealRequest = null, activeHighlight = null }: { results: SubAgentResult[]; revealRequest?: AgentTextRevealRequest | null; activeHighlight?: AgentTextHighlight | null }) {
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
          <SubAgentSummaryRow key={result.agent_id} result={result} revealRequest={revealRequest} activeHighlight={activeHighlight} />
        ))}
      </div>
    </div>
  );
}

// =====================================================================// Sub-Agent Status (live progress indicator)
// =====================================================================
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
}: {
  agentId: string;
  task: string;
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
      viewer.open({ agentId, task });
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
  }, [usePanel, viewer, agentId, task, navigate]);

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
