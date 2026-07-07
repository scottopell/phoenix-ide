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

export interface ToolStripItem {
  /** Tool name as it appears on the content block (e.g. `bash`, `patch`). */
  name: string;
  /** The tool_use block id; used to key the summary and to target expansion. */
  toolId: string;
  /** Short input-derived description that distinguishes repeated tool calls. */
  inputSummary: string;
  /** Short result-derived description, when a cheap summary exists. */
  resultSummary: string | null;
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
): ToolStripItem[] {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const items: ToolStripItem[] = [];
  for (const block of blocks) {
    if (block.type !== 'tool_use') continue;
    const name = block.name || 'tool';
    if (name === 'think') continue;
    const toolId = block.id || '';
    const result = toolId ? toolResultsByUseId.get(toolId) : undefined;
    const resultContent = result?.content as ToolResultContent | undefined;
    const isError = !!(resultContent?.is_error || resultContent?.error);
    items.push({
      name,
      toolId,
      inputSummary: summarizeToolInput(name, (block.input || {}) as Record<string, unknown>, block.display),
      resultSummary: summarizeToolResult(name, result),
      isSubAgent: name === 'spawn_agents',
      hasResult: result !== undefined,
      isError,
    });
  }
  return items;
}
