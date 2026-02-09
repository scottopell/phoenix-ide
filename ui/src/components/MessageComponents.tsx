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

import { useState, useEffect } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { Message, ContentBlock, ToolResultContent, ConversationState, SubAgentResult, SseEventType, SseEventData, SseInitData, SseMessageData } from '../api';
import { api } from '../api';
import type { QueuedMessage } from '../hooks';
import { escapeHtml } from '../utils';
import { CopyButton } from './CopyButton';
import { PatchFileSummary, containsUnifiedDiff } from './PatchFileSummary';

// ============================================================================
// Helper functions
// ============================================================================

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

function formatToolInput(name: string, input: Record<string, unknown>): { display: string; isMultiline: boolean } {
  switch (name) {
    case 'bash': {
      const cmd = String(input.command || '');
      return { display: `$ ${cmd}`, isMultiline: cmd.includes('\n') };
    }
    case 'think': {
      const thoughts = String(input.thoughts || '');
      return { display: thoughts, isMultiline: thoughts.includes('\n') };
    }
    case 'patch': {
      const path = String(input.path || '');
      const patches = input.patches as Array<{ operation?: string }> | undefined;
      const op = patches?.[0]?.operation || 'modify';
      const count = patches?.length || 1;
      const summary = count > 1 ? `${path}: ${count} patches` : `${path}: ${op}`;
      return { display: summary, isMultiline: false };
    }
    case 'keyword_search': {
      const query = String(input.query || '');
      const terms = (input.search_terms as string[]) || [];
      const termsStr = terms.length > 0 ? terms.slice(0, 3).join(', ') + (terms.length > 3 ? '...' : '') : '';
      return { display: termsStr ? `"${query}" [${termsStr}]` : query, isMultiline: false };
    }
    case 'read_image': {
      const path = String(input.path || '');
      return { display: path, isMultiline: false };
    }
    default: {
      const str = JSON.stringify(input, null, 2);
      return { display: str, isMultiline: str.includes('\n') };
    }
  }
}

// ============================================================================
// User Message Components
// ============================================================================

export function UserMessage({ message }: { message: Message }) {
  const content = message.content as { text?: string; images?: { data: string; media_type: string }[] };
  const text = content.text || (typeof message.content === 'string' ? message.content : '');
  const images = content.images || [];
  const timestamp = message.created_at;

  return (
    <div className="message user" data-sequence-id={message.sequence_id}>
      <div className="message-header">
        <span className="message-sender">You</span>
        {timestamp && (
          <span className="message-time" title={new Date(timestamp).toLocaleString()}>
            {formatMessageTime(timestamp)}
          </span>
        )}
        <span className="message-status sent" title="Sent">✓</span>
      </div>
      <div className="message-content">
        {escapeHtml(text)}
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

export function QueuedUserMessage({ message, onRetry }: { message: QueuedMessage; onRetry: (localId: string) => void }) {
  const isFailed = message.status === 'failed';
  const isSending = message.status === 'sending';

  return (
    <div className={`message user ${isFailed ? 'failed' : ''}`}>
      <div className="message-header">
        <span className="message-sender">You</span>
        {isSending && (
          <span className="message-status sending" title="Sending...">
            <span className="sending-spinner">⏳</span>
          </span>
        )}
        {isFailed && (
          <span 
            className="message-status failed" 
            title="Failed - tap to retry"
            onClick={() => onRetry(message.localId)}
            style={{ cursor: 'pointer' }}
          >
            ⚠️
          </span>
        )}
      </div>
      <div className="message-content">
        {escapeHtml(message.text)}
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

export function AgentMessage({
  message,
  toolResults,
  onOpenFile,
}: {
  message: Message;
  toolResults: Map<string, Message>;
  onOpenFile?: (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void;
}) {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const timestamp = message.created_at;

  return (
    <div className="message agent" data-sequence-id={message.sequence_id}>
      <div className="message-header">
        <span className="message-sender">Phoenix</span>
        {timestamp && (
          <span className="message-time" title={new Date(timestamp).toLocaleString()}>
            {formatMessageTime(timestamp)}
          </span>
        )}
      </div>
      <div className="message-content">
        {blocks.map((block, i) => {
          if (block.type === 'text') {
            return (
              <div key={i} className="agent-text-block">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={{
                    code: ({ inline, className, children, ...props }: any) => {
                      const match = /language-(\w+)/.exec(className || '');
                      return !inline && match ? (
                        <SyntaxHighlighter
                          style={oneDark}
                          language={match[1]}
                          PreTag="div"
                          {...props}
                        >
                          {String(children).replace(/\n$/, '')}
                        </SyntaxHighlighter>
                      ) : (
                        <code className={className} {...props}>
                          {children}
                        </code>
                      );
                    },
                  }}
                >
                  {block.text || ''}
                </ReactMarkdown>
              </div>
            );
          } else if (block.type === 'tool_use') {
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
// Tool Use Block
// ============================================================================

interface ToolUseBlockProps {
  block: ContentBlock;
  result?: Message;
  onOpenFile?: (filePath: string, modifiedLines: Set<number>, firstModifiedLine: number) => void;
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

export function ToolUseBlock({ block, result, onOpenFile }: ToolUseBlockProps) {
  const name = block.name || 'tool';
  const input = block.input || {};
  const toolId = block.id || '';

  // Format the input display based on tool type
  const { display: inputDisplay, isMultiline: inputIsMultiline } = formatToolInput(name, input as Record<string, unknown>);

  // Get the paired result if available
  let resultContent: ToolResultContent | null = null;
  if (result) {
    resultContent = result.content as ToolResultContent;
  }

  const resultText = resultContent?.content || resultContent?.result || resultContent?.error || '';
  const isError = resultContent?.is_error || !!resultContent?.error;
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

  // Determine if output should be auto-expanded
  const shouldAutoExpand = resultLength > 0 && resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;
  const [outputExpanded, setOutputExpanded] = useState(shouldAutoExpand);

  // For display, truncate very long outputs even when expanded
  const maxDisplayLen = 5000;
  const displayResult = resultText.length > maxDisplayLen 
    ? resultText.slice(0, maxDisplayLen) + `\n... (${resultText.length - maxDisplayLen} more chars)`
    : resultText;

  // Preview for collapsed state
  const previewLen = 100;
  const previewText = resultText.length > previewLen 
    ? resultText.slice(0, previewLen).split('\n')[0] + '...'
    : resultText.split('\n')[0];

  const hasOutput = resultContent !== null;
  const isShortOutput = resultLength < OUTPUT_AUTO_EXPAND_THRESHOLD;

  // Get the raw input for copying (not the formatted display)
  const rawInput = name === 'bash' ? String(input.command || '') : 
                   name === 'think' ? String(input.thoughts || '') :
                   JSON.stringify(input, null, 2);

  return (
    <div className="tool-block" data-tool-id={toolId}>
      {/* Tool header with name */}
      <div className="tool-block-header">
        <span className="tool-block-name">{name}</span>
        {hasOutput && (
          <span className={`tool-block-status ${isError ? 'error' : 'success'}`}>
            {isError ? '✗' : '✓'}
          </span>
        )}
      </div>

      {/* Tool input - always visible */}
      <div className={`tool-block-input ${inputIsMultiline ? 'multiline' : ''}`}>
        {inputDisplay}
        <CopyButton text={rawInput} title="Copy command" />
      </div>

      {/* Tool output - collapsible for long outputs */}
      {hasOutput && (
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
            // Long output: collapsible
            <>
              <div 
                className="tool-block-output-header" 
                onClick={() => setOutputExpanded(!outputExpanded)}
              >
                <span className="tool-block-output-chevron">{outputExpanded ? '▼' : '▶'}</span>
                <span className="tool-block-output-label">
                  {outputExpanded ? 'output' : previewText}
                </span>
                <span className="tool-block-output-size">({resultLength.toLocaleString()} chars)</span>
                <CopyButton text={resultText} title="Copy output" />
              </div>
              {outputExpanded && (
                <div className="tool-block-output-content">
                  {displayResult}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* Patch file summary (REQ-PF-014) */}
      {name === 'patch' && resultText && containsUnifiedDiff(resultText) && onOpenFile && (
        <PatchFileSummary patchOutput={resultText} onFileClick={onOpenFile} />
      )}

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
    (data as Record<string, unknown>).type === 'subagent_summary' &&
    Array.isArray((data as Record<string, unknown>).results)
  );
}

/** Single completed sub-agent row with expandable conversation view */
function SubAgentSummaryRow({ result }: { result: SubAgentResult }) {
  const [conversationExpanded, setConversationExpanded] = useState(false);
  const isError = result.outcome.type === 'failure';
  const icon = isError ? '✗' : '✓';
  const resultText = getOutcomeText(result.outcome);

  return (
    <div className={`subagent-summary-row ${isError ? 'error' : ''}`}>
      <div 
        className="subagent-summary-header"
        onClick={() => setConversationExpanded(!conversationExpanded)}
      >
        <span className={`subagent-summary-icon ${isError ? 'error' : 'success'}`}>{icon}</span>
        <span className="subagent-summary-task" title={result.task}>
          {truncate(result.task, 60)}
        </span>
        <span className="subagent-summary-outcome">
          {truncate(resultText, 50)}
        </span>
        <span className="subagent-summary-expand">
          {conversationExpanded ? '▲' : '▼'}
        </span>
      </div>
      {conversationExpanded && (
        <div className="subagent-embedded-view">
          <EmbeddedSubagentView agentId={result.agent_id} />
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
          {successCount > 0 && <span className="success">✓ {successCount}</span>}
          {failCount > 0 && <span className="error">✗ {failCount}</span>}
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
// Embedded Subagent View (simplified message list for expanded subagent)
// ============================================================================

/** Renders a simplified view of subagent conversation messages */
function EmbeddedSubagentView({ agentId }: { agentId: string }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let eventSource: EventSource | null = null;
    let mounted = true;

    // Connect to subagent's SSE stream
    eventSource = api.streamConversation(agentId, (eventType: SseEventType, data: SseEventData) => {
      if (!mounted) return;

      switch (eventType) {
        case 'init': {
          const initData = data as SseInitData;
          setMessages(initData.messages);
          setLoading(false);
          break;
        }
        case 'message': {
          const msgData = data as SseMessageData;
          setMessages(prev => [...prev, msgData.message]);
          break;
        }
        case 'disconnected':
          // Connection closed, likely subagent finished
          break;
      }
    });

    eventSource.onerror = () => {
      if (mounted) {
        // Don't show error for completed subagents (SSE closes normally)
        if (loading) {
          setError('Failed to load subagent conversation');
        }
        setLoading(false);
      }
    };

    return () => {
      mounted = false;
      eventSource?.close();
    };
  }, [agentId, loading]);

  if (loading) {
    return <div className="embedded-subagent-loading">Loading conversation...</div>;
  }

  if (error) {
    return <div className="embedded-subagent-error">{error}</div>;
  }

  // Build tool results map
  const toolResults = new Map<string, Message>();
  for (const msg of messages) {
    if (msg.message_type === 'tool') {
      const content = msg.content as ToolResultContent;
      if (content.tool_use_id) {
        toolResults.set(content.tool_use_id, msg);
      }
    }
  }

  return (
    <div className="embedded-subagent-messages">
      {messages.map((msg) => {
        if (msg.message_type === 'user') {
          // Show user messages (the task prompt)
          const content = msg.content as { text?: string };
          return (
            <div key={msg.message_id} className="embedded-msg user">
              <span className="embedded-msg-label">Task:</span>
              <span className="embedded-msg-text">{content.text || ''}</span>
            </div>
          );
        }

        if (msg.message_type === 'agent') {
          // Show agent messages with text and tool blocks
          const blocks = Array.isArray(msg.content) ? (msg.content as ContentBlock[]) : [];
          return (
            <div key={msg.message_id} className="embedded-msg agent">
              {blocks.map((block, i) => {
                if (block.type === 'text' && block.text) {
                  return (
                    <div key={i} className="embedded-agent-text">
                      {truncate(block.text, 500)}
                    </div>
                  );
                }
                if (block.type === 'tool_use') {
                  const toolName = block.name || 'tool';
                  const toolResult = toolResults.get(block.id || '');
                  const resultContent = toolResult?.content as ToolResultContent | undefined;
                  const isToolError = resultContent?.is_error || !!resultContent?.error;
                  const toolOutput = resultContent?.content || resultContent?.result || resultContent?.error || '';

                  return (
                    <div key={block.id || i} className="embedded-tool-block">
                      <div className="embedded-tool-header">
                        <span className="embedded-tool-name">{toolName}</span>
                        {toolResult && (
                          <span className={`embedded-tool-status ${isToolError ? 'error' : 'success'}`}>
                            {isToolError ? '✗' : '✓'}
                          </span>
                        )}
                      </div>
                      {toolOutput && (
                        <div className={`embedded-tool-output ${isToolError ? 'error' : ''}`}>
                          {truncate(toolOutput, 200)}
                        </div>
                      )}
                    </div>
                  );
                }
                return null;
              })}
            </div>
          );
        }

        return null;
      })}
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

/** Single completed sub-agent with expandable result */
function CompletedSubAgent({ result }: { result: SubAgentResult }) {
  const [expanded, setExpanded] = useState(false);
  const isError = result.outcome.type === 'failure';
  const icon = isError ? '✗' : '✓';
  const resultText = getOutcomeText(result.outcome);
  const hasLongResult = resultText.length > 100;

  return (
    <div className={`subagent-item completed ${isError ? 'error' : ''}`}>
      <div className="subagent-item-header" onClick={() => hasLongResult && setExpanded(!expanded)}>
        <span className="subagent-icon">{icon}</span>
        <span className="subagent-label" title={result.task}>
          {truncate(result.task, 50)}
        </span>
        {hasLongResult && (
          <span className="subagent-expand-toggle">
            {expanded ? '▲' : '▼'}
          </span>
        )}
      </div>
      <div className={`subagent-result ${expanded ? 'expanded' : ''}`}>
        {expanded ? resultText : truncate(resultText, 100)}
      </div>
    </div>
  );
}

export function SubAgentStatus({ stateData }: { stateData: ConversationState }) {
  const pending = stateData.pending ?? [];
  const completed = stateData.completed_results ?? [];
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
          </div>
        ))}
      </div>
    </div>
  );
}
