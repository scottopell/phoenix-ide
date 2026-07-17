import type { BashToolInput } from '../generated/sse';
import { formatCommissionReviewInput } from '../features/commissionReview/model';

export function cleanToolThoughts(raw: string): string {
  let text = raw.replace(/^\s*<thinking>\s*/i, '');
  const closingIdx = text.search(/<\/thinking>/i);
  if (closingIdx !== -1) text = text.slice(0, closingIdx);
  return text.trim();
}

export function truncateToolInputValue(value: string, max = 40): string {
  return value.length > max ? `${value.slice(0, max)}…` : value;
}

function isFiniteInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value);
}

export function isBashToolInput(input: Record<string, unknown>): input is BashToolInput {
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

function formatModernBashInput(input: BashToolInput, displayOverride?: string): ToolInputDisplay {
  switch (input.op) {
    case 'run': {
      const cmd = input.cmd || '';
      if (!cmd) return { display: 'bash run <missing cmd>', isMultiline: false };
      const displayCmd = displayOverride || cmd;
      const waitSuffix = typeof input.wait_seconds === 'number' ? ` · wait ${input.wait_seconds}s` : '';
      return { display: `$ ${displayCmd}${waitSuffix}${readWindowSuffix(input)}`, isMultiline: cmd.includes('\n') };
    }
    case 'peek':
      return { display: `peek ${input.handle || '<missing handle>'}${readWindowSuffix(input)}`, isMultiline: false };
    case 'wait': {
      const waitSuffix = typeof input.wait_seconds === 'number' ? ` (up to ${input.wait_seconds}s)` : '';
      return { display: `wait ${input.handle || '<missing handle>'}${waitSuffix}${readWindowSuffix(input)}`, isMultiline: false };
    }
    case 'kill':
      return { display: `kill ${input.handle || '<missing handle>'} (${input.signal || 'TERM'})`, isMultiline: false };
  }
}

export function bashInputCopyText(input: Record<string, unknown>): string {
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

function formatBrowserInput(name: string, input: Record<string, unknown>): string {
  switch (name) {
    case 'browser_navigate': return `→ ${String(input['url'] || '')}`;
    case 'browser_eval': return `eval: ${truncateToolInputValue(String(input['expression'] || '').replace(/\s+/g, ' ').trim(), 80)}`;
    case 'browser_take_screenshot': return input['selector'] ? `screenshot of "${String(input['selector'])}"` : 'screenshot';
    case 'browser_recent_console_logs': return input['limit'] !== undefined ? `console logs (${String(input['limit'])})` : 'console logs';
    case 'browser_clear_console_logs': return 'clear console';
    case 'browser_resize': return `resize ${String(input['width'])}x${String(input['height'])}`;
    case 'browser_wait_for_selector': return `wait "${String(input['selector'] || '')}"${input['visible'] === true ? ' (visible)' : ''}`;
    case 'browser_click': return `click "${String(input['selector'] || '')}"`;
    case 'browser_type': return `${input['clear'] === true ? 'replace' : 'type'} "${String(input['selector'] || '')}" = "${truncateToolInputValue(String(input['text'] || ''))}"`;
    case 'browser_key_press': {
      const key = String(input['key'] || '');
      const modifiers = (input['modifiers'] as string[]) || [];
      return `key: ${modifiers.length > 0 ? `${modifiers.join('+')}+${key}` : key}`;
    }
    case 'browser_profile': {
      const action = String(input['action'] || '');
      if (action === 'run_scenario') {
        const steps = Array.isArray(input['steps']) ? input['steps'].length : 0;
        const throttle = input['throttle_rate'] != null ? `, throttle ${String(input['throttle_rate'])}x` : '';
        const reset = input['reset'];
        const resetText = reset === 'none' ? ', reset:none' : reset && typeof reset === 'object' ? `, reset:${String((reset as Record<string, unknown>)['kind'] ?? '?')}` : '';
        return `profile: scenario (${steps} steps × ${String(input['runs'] ?? 1)} runs, ${String(input['warmup'] ?? 1)} warmup${throttle}${resetText}${input['gc_per_run'] === false ? ', gc:off' : ''})`;
      }
      if (action === 'throttle') return `profile: throttle ${String(input['rate'] ?? '')}x`;
      if (action === 'trace_start') return `profile: trace_start${input['categories'] ? ` [${String(input['categories'])}]` : ''}`;
      if (action === 'heap_snapshot') return input['baseline'] ? 'profile: heap_snapshot (diff)' : 'profile: heap_snapshot';
      return action ? `profile: ${action}` : 'profile';
    }
    default: return JSON.stringify(input, null, 2);
  }
}

export function skillCommandFromInput(input: Record<string, unknown>): string {
  const skillName = String(input['skill_name'] || 'skill').replace(/^\/+/, '');
  const args = String(input['args'] || '').trim();
  return args ? `/${skillName} ${args}` : `/${skillName}`;
}

export function skillResultVisibleText(resultText: string): string {
  const sourceMatch = /^Base directory for this skill:\s*(.+)$/m.exec(resultText);
  const body = resultText.replace(/^Base directory for this skill:\s*.+\n?/m, '').trim();
  const snippet = body.split('\n').find((line) => line.trim().length > 0)?.replace(/^#\s*/, '').trim() ?? '';
  return [sourceMatch?.[1] ? `${sourceMatch[1]}/SKILL.md` : '', snippet].filter(Boolean).join('\n');
}

export interface ToolInputDisplay {
  display: string;
  isMultiline: boolean;
}

export function formatToolInput(name: string, input: Record<string, unknown>, displayOverride?: string): ToolInputDisplay {
  if (name === 'commission_review') return formatCommissionReviewInput(input);
  switch (name) {
    case 'skill': {
      const display = skillCommandFromInput(input);
      return { display, isMultiline: false };
    }
    case 'bash': {
      if (isBashToolInput(input)) return formatModernBashInput(input, displayOverride);
      const legacyCommand = input['op'] === undefined ? String(input['command'] || input['cmd'] || '') : '';
      if (legacyCommand) return { display: `$ ${displayOverride || legacyCommand}`, isMultiline: legacyCommand.includes('\n') };
      return { display: `bash ${JSON.stringify(input)}`, isMultiline: false };
    }
    case 'tmux': return { display: `tmux ${((input['args'] as unknown[] | undefined) ?? []).map(String).join(' ')}`, isMultiline: false };
    case 'think': {
      const display = cleanToolThoughts(String(input['thoughts'] || ''));
      return { display, isMultiline: display.includes('\n') };
    }
    case 'patch': {
      const patches = input['patches'] as Array<{ operation?: string }> | undefined;
      const count = patches?.length || 1;
      return { display: count > 1 ? `${String(input['path'] || '')}: ${count} patches` : `${String(input['path'] || '')}: ${patches?.[0]?.operation || 'modify'}`, isMultiline: false };
    }
    case 'keyword_search': {
      const query = String(input['query'] || '');
      const terms = (input['search_terms'] as string[]) || [];
      const termsText = terms.length > 0 ? `${terms.slice(0, 3).join(', ')}${terms.length > 3 ? '...' : ''}` : '';
      return { display: termsText ? `"${query}" [${termsText}]` : query, isMultiline: false };
    }
    case 'read_image': return { display: String(input['path'] || ''), isMultiline: false };
    case 'read_file': {
      const path = String(input['path'] || '');
      const offset = input['offset'] as number | undefined;
      const limit = input['limit'] as number | undefined;
      const display = offset !== undefined || limit !== undefined
        ? limit !== undefined ? `${path}:${offset ?? 1}-${(offset ?? 1) + limit - 1}` : `${path}:${offset ?? 1}+`
        : path;
      return { display, isMultiline: false };
    }
    case 'spawn_agents': {
      const count = ((input['tasks'] as unknown[] | undefined) ?? []).length;
      return { display: `${count} parallel task${count === 1 ? '' : 's'}`, isMultiline: false };
    }
    case 'ask_user_question': {
      const questions = (input['questions'] as Array<{ question?: string; options?: unknown[] }> | undefined) ?? [];
      const text = String(questions[0]?.question || '').replace(/\s+/g, ' ').trim();
      const suffix = questions.length > 1 ? ` [+${questions.length - 1} more]` : (questions[0]?.options?.length ?? 0) > 0 ? ` [${questions[0]!.options!.length} options]` : '';
      return { display: `"${truncateToolInputValue(text, 80)}"${suffix}`, isMultiline: false };
    }
    case 'search': {
      let display = `"${String(input['pattern'] || '')}"`;
      if (input['path']) display += ` in ${String(input['path'])}`;
      if (input['include']) display += ` (${String(input['include'])})`;
      return { display, isMultiline: false };
    }
    default: {
      const display = name.startsWith('browser_') ? formatBrowserInput(name, input) : JSON.stringify(input, null, 2);
      return { display, isMultiline: display.includes('\n') };
    }
  }
}
