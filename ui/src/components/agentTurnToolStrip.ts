// Pure derivation of a compact-mode tool pill strip from an agent turn's
// content blocks + its paired tool results.
//
// This is presentational support for the conversation density feature: it
// turns the tool_use blocks already present on an `agent_turn` render unit
// into the lightweight `{ name, toolId, isSubAgent, hasResult, isError }`
// descriptors the inline pill strip paints. It reads ONLY the turn's own
// data (content blocks + `toolResultsByUseId`), never phase state — the
// source of truth for what a turn did is the turn itself.

import type { ContentBlock, Message, ToolResultContent } from '../api';
import type { BashToolProgress } from '../generated/sse';

export interface ToolStripItem {
  /** Tool name as it appears on the content block (e.g. `bash`, `patch`). */
  name: string;
  /** The tool_use block id; used to key the summary and to target expansion. */
  toolId: string;
  /** Short input-derived description that distinguishes repeated tool calls. */
  inputSummary: string;
  /** Short result-derived description, when a cheap summary exists. */
  resultSummary: string | null;
  /** Compact bash cards: operation/command identity rendered above the tail. */
  commandIdentity: string | null;
  /** Compact bash cards: final status/duration badge text. */
  finalStatus: string | null;
  /** Server timestamp used to advance compact in-flight elapsed time. */
  startedAtMs: number | null;
  /** Compact bash cards: bounded final output tail from the existing result payload. */
  outputTail: string | null;
  /** spawn_agents launches sub-agents — colored distinctly in the strip. */
  isSubAgent: boolean;
  /** Whether a paired tool result has landed for this tool yet. */
  hasResult: boolean;
  /** Whether the paired result reported an error. */
  isError: boolean;
}

const SUMMARY_LIMIT = 96;

function truncate(text: string, maxLen = SUMMARY_LIMIT): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  return flat.length > maxLen ? `${flat.slice(0, maxLen - 1)}…` : flat;
}

function stringInput(input: Record<string, unknown>, key: string): string {
  const value = input[key];
  return typeof value === 'string' ? value : value == null ? '' : String(value);
}

function firstScalarInput(input: Record<string, unknown>): string {
  for (const [key, value] of Object.entries(input)) {
    if (value == null) continue;
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
      const text = String(value).trim();
      if (text) return `${key}: ${text}`;
    }
  }
  const json = JSON.stringify(input);
  return json && json !== '{}' ? json : 'no input';
}

function summarizePatchInput(input: Record<string, unknown>): string {
  const path = stringInput(input, 'path');
  const patches = Array.isArray(input['patches']) ? input['patches'] as Array<{ operation?: unknown }> : [];
  const count = patches.length;
  if (count === 0) return path || 'patch';
  const operations = Array.from(new Set(patches.map((patch) => String(patch.operation ?? 'modify'))));
  const opText = operations.length === 1 ? operations[0] : `${operations.length} ops`;
  return path ? `${path}: ${count} ${count === 1 ? 'patch' : 'patches'} (${opText})` : `${count} ${count === 1 ? 'patch' : 'patches'} (${opText})`;
}

function summarizeBrowserInput(name: string, input: Record<string, unknown>): string {
  switch (name) {
    case 'browser_navigate': return stringInput(input, 'url') || 'navigate';
    case 'browser_click': return stringInput(input, 'selector') || 'click';
    case 'browser_type': {
      const selector = stringInput(input, 'selector');
      const text = stringInput(input, 'text');
      return selector && text ? `${selector}: ${text}` : selector || text || 'type';
    }
    case 'browser_wait_for_selector': return stringInput(input, 'selector') || 'wait for selector';
    case 'browser_eval': return stringInput(input, 'expression') || 'evaluate JavaScript';
    case 'browser_take_screenshot': return stringInput(input, 'selector') || 'page screenshot';
    case 'browser_recent_console_logs': return 'console logs';
    default: return firstScalarInput(input);
  }
}

function summarizeToolInput(name: string, input: Record<string, unknown>, display?: string): string {
  if (display) return truncate(display.replace(/^\$\s*/, ''));
  switch (name) {
    case 'search': {
      const pattern = stringInput(input, 'pattern');
      const path = stringInput(input, 'path');
      const include = stringInput(input, 'include');
      return truncate([pattern || 'search', path ? `in ${path}` : '', include ? `(${include})` : ''].filter(Boolean).join(' '));
    }
    case 'read_file': {
      const path = stringInput(input, 'path');
      const offset = typeof input['offset'] === 'number' ? input['offset'] as number : null;
      const limit = typeof input['limit'] === 'number' ? input['limit'] as number : null;
      if (offset != null || limit != null) {
        const start = offset ?? 1;
        const end = limit != null ? start + limit - 1 : null;
        return end != null ? `${path}:${start}-${end}` : `${path}:${start}+`;
      }
      return path || 'read file';
    }
    case 'bash': return truncate(stringInput(input, 'cmd') || stringInput(input, 'command') || firstScalarInput(input).replace(/^cmd: /, ''));
    case 'patch': return truncate(summarizePatchInput(input));
    case 'keyword_search': {
      const query = stringInput(input, 'query');
      const terms = Array.isArray(input['search_terms']) ? input['search_terms'].map(String).slice(0, 3).join(', ') : '';
      return truncate(terms ? `${query} [${terms}]` : query || 'keyword search');
    }
    case 'spawn_agents': {
      const tasks = Array.isArray(input['tasks']) ? input['tasks'] : [];
      return `${tasks.length} parallel ${tasks.length === 1 ? 'task' : 'tasks'}`;
    }
    case 'skill': return truncate(stringInput(input, 'skill_name') || 'skill');
    case 'propose_task': return truncate(stringInput(input, 'task_file') || stringInput(input, 'slug') || firstScalarInput(input));
    case 'browser_navigate':
    case 'browser_click':
    case 'browser_type':
    case 'browser_wait_for_selector':
    case 'browser_eval':
    case 'browser_take_screenshot':
    case 'browser_recent_console_logs':
      return truncate(summarizeBrowserInput(name, input));
    default:
      return truncate(firstScalarInput(input));
  }
}

function resultText(result: Message | undefined): string {
  if (!result) return '';
  const content = result.content as ToolResultContent | undefined;
  return content?.content || content?.result || content?.error || '';
}

function tryParseJson(text: string): Record<string, unknown> | null {
  if (!text) return null;
  try {
    const parsed = JSON.parse(text);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, unknown> : null;
  } catch {
    return null;
  }
}

function summarizeSearchResult(text: string): string | null {
  const trimmed = text.trim();
  if (trimmed === 'No matches found.') return 'no matches';
  const files = new Set<string>();
  let matches = 0;
  for (const line of text.split('\n')) {
    if (!line.trim() || line.startsWith('[')) continue;
    const match = /^(.+?):(\d+):/.exec(line);
    if (!match) continue;
    matches += 1;
    files.add(match[1]!);
  }
  if (matches === 0) return null;
  return `${matches} ${matches === 1 ? 'match' : 'matches'} in ${files.size} ${files.size === 1 ? 'file' : 'files'}`;
}

function summarizeKeywordSearchResult(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed || trimmed === 'No matches found for the given search terms.' || trimmed.startsWith('No relevant files found')) {
    return 'no relevant files';
  }
  const hits = text.split('\n').filter((line) => /^([^:\s][^:]*?):\s+(.+)$/.test(line)).length;
  return hits > 0 ? `${hits} relevant ${hits === 1 ? 'file' : 'files'}` : null;
}

function summarizeConsoleLogs(text: string): string | null {
  try {
    const parsed = JSON.parse(text);
    if (!Array.isArray(parsed)) return null;
    const counts = new Map<string, number>();
    for (const entry of parsed) {
      const level = entry && typeof entry === 'object' && typeof entry.level === 'string' ? entry.level : 'log';
      counts.set(level, (counts.get(level) ?? 0) + 1);
    }
    if (parsed.length === 0) return 'no console entries';
    const important = ['error', 'warning', 'info', 'log']
      .filter((level) => counts.has(level))
      .map((level) => `${counts.get(level)} ${level}`)
      .join(', ');
    return important || `${parsed.length} entries`;
  } catch {
    return null;
  }
}
function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const totalSeconds = Math.floor(ms / 1000);
  if (totalSeconds < 10) return `${(ms / 1000).toFixed(1)}s`;
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`;
}

function summarizeBashInputIdentity(input: Record<string, unknown>, display?: string): string {
  const op = stringInput(input, 'op');
  if (op === 'run') return truncate(display || stringInput(input, 'cmd') || 'run');
  if (op === 'wait') return truncate(`wait ${stringInput(input, 'handle') || '<missing handle>'}`);
  if (op === 'peek') return truncate(`peek ${stringInput(input, 'handle') || '<missing handle>'}`);
  if (op === 'kill') {
    const handle = stringInput(input, 'handle') || '<missing handle>';
    const signal = stringInput(input, 'signal') || 'TERM';
    return truncate(`kill ${handle} (${signal})`);
  }
  return truncate(display || stringInput(input, 'cmd') || stringInput(input, 'command') || firstScalarInput(input).replace(/^cmd: /, ''));
}

function bashStatusLabel(parsed: Record<string, unknown>): string | null {
  if (typeof parsed['error'] === 'string') return 'error';
  const status = typeof parsed['status'] === 'string' ? parsed['status'] : '';
  const exitCode = parsed['exit_code'];
  const finalCause = stringInput(parsed, 'final_cause');
  switch (status) {
    case 'running': return 'running';
    case 'still_running': return 'still running';
    case 'kill_pending_kernel': return 'kill pending';
    case 'exited': return exitCode == null ? 'exited' : `exit ${String(exitCode)}`;
    case 'killed': return exitCode == null ? 'killed' : `killed ${String(exitCode)}`;
    case 'tombstoned': return finalCause ? `tombstoned · ${finalCause}` : 'tombstoned';
    default: return status ? status.replace(/_/g, ' ') : null;
  }
}

function summarizeBashOutputTail(parsed: Record<string, unknown>): string | null {
  const lines = Array.isArray(parsed['lines']) ? parsed['lines'] : [];
  const bytes = lines
    .map((line) => line && typeof line === 'object' && typeof (line as { bytes?: unknown }).bytes === 'string'
      ? (line as { bytes: string }).bytes
      : '')
    .filter((line) => line.trim().length > 0);
  const partial = typeof parsed['partial'] === 'string' ? parsed['partial'].trim() : '';
  const tailParts = bytes.slice(-2);
  if (partial) tailParts.push(`${partial} …`);
  if (tailParts.length === 0) return null;
  const localTailTruncated = bytes.length > 2;
  const prefix = parsed['truncated_before'] === true || localTailTruncated ? '… ' : '';
  return truncate(`${prefix}${tailParts.join(' · ')}`, 140);
}
function summarizeBashCompactCard(
  input: Record<string, unknown>,
  result: Message | undefined,
  progress: BashToolProgress | undefined,
  display?: string,
): Pick<ToolStripItem, 'commandIdentity' | 'finalStatus' | 'outputTail'> {
  const commandIdentity = summarizeBashInputIdentity(input, display);
  if (!result) {
    const liveLines = progress?.lines.map((line) => line.text) ?? [];
    if (progress?.partial) liveLines.push(progress.partial);
    const nonEmptyLiveLines = liveLines.filter((line) => line.trim());
    const liveTailTruncated = progress?.truncated_before === true || nonEmptyLiveLines.length > 2;
    const liveTail = `${liveTailTruncated ? '… ' : ''}${nonEmptyLiveLines.slice(-2).join(' · ')}`;
    return {
      commandIdentity,
      finalStatus: progress ? 'running' : null,
      outputTail: liveTail ? truncate(liveTail, 140) : null,
    };
  }
  const text = resultText(result);
  const parsed = tryParseJson(text);
  if (!parsed) {
    return {
      commandIdentity,
      finalStatus: null,
      outputTail: text.trim() ? truncate(text.trim(), 140) : null,
    };
  }
  const status = bashStatusLabel(parsed);
  const displayData = result.display_data as Record<string, unknown> | undefined;
  const durationMs = typeof parsed['duration_ms'] === 'number'
    ? parsed['duration_ms']
    : typeof displayData?.['duration_ms'] === 'number'
      ? displayData['duration_ms']
      : null;
  return {
    commandIdentity,
    finalStatus: status ? `${status}${durationMs !== null ? ` · ${formatDuration(durationMs)}` : ''}` : durationMs !== null ? formatDuration(durationMs) : null,
    outputTail: summarizeBashOutputTail(parsed),
  };
}

function summarizeToolResult(name: string, result: Message | undefined): string | null {
  if (!result) return null;
  const text = resultText(result);
  switch (name) {
    case 'search': return summarizeSearchResult(text);
    case 'keyword_search': return summarizeKeywordSearchResult(text);
    case 'read_file': {
      if (!text) return 'empty';
      const lineCount = text.split('\n').length;
      return `${lineCount} ${lineCount === 1 ? 'line' : 'lines'}`;
    }
    case 'bash': {
      const parsed = tryParseJson(text);
      if (!parsed) return null;
      const status = typeof parsed['status'] === 'string' ? parsed['status'] : '';
      const exitCode = parsed['exit_code'];
      return status === 'exited' && exitCode != null ? `exited ${String(exitCode)}` : status.replace(/_/g, ' ') || null;
    }
    case 'browser_recent_console_logs': return summarizeConsoleLogs(text);
    default: return null;
  }
}

/**
 * Derive the compact tool strip for a single agent message. `think` blocks
 * are excluded — they are model reasoning, not actions, and already render
 * as their own self-collapsing aside. Returns one item per remaining
 * tool_use block, in document order.
 */
export function deriveToolStripItems(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
  liveBashProgress: Readonly<Record<string, { progress: BashToolProgress }>> = {},
): ToolStripItem[] {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const toolStarts = (message.display_data as Record<string, unknown> | undefined)?.['tool_starts'] as Record<string, unknown> | undefined;
  const items: ToolStripItem[] = [];
  for (const block of blocks) {
    if (block.type !== 'tool_use') continue;
    const name = block.name || 'tool';
    if (name === 'think') continue;
    const toolId = block.id || '';
    const result = toolId ? toolResultsByUseId.get(toolId) : undefined;
    const resultContent = result?.content as ToolResultContent | undefined;
    const isError = !!(resultContent?.is_error || resultContent?.error);
    const input = (block.input || {}) as Record<string, unknown>;
    const bashCompact = name === 'bash'
      ? summarizeBashCompactCard(input, result, liveBashProgress[toolId]?.progress, block.display)
      : { commandIdentity: null, finalStatus: null, outputTail: null };
    items.push({
      name,
      toolId,
      inputSummary: summarizeToolInput(name, input, block.display),
      resultSummary: summarizeToolResult(name, result),
      commandIdentity: bashCompact.commandIdentity,
      finalStatus: bashCompact.finalStatus,
      startedAtMs: typeof toolStarts?.[toolId] === 'number' ? toolStarts[toolId] : null,
      outputTail: bashCompact.outputTail,
      isSubAgent: name === 'spawn_agents',
      hasResult: result !== undefined,
      isError,
    });
  }
  return items;
}
