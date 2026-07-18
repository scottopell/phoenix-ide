import type { ContentBlock, Message, ToolResultContent } from '../../api';
import type { BashToolProgress } from '../../generated/sse';
import type { StreamingBuffer } from '../../conversation/atom';
import type { DiffSection } from '../../contexts/ReviewNotesContext';
import type { QueuedMessage } from '../../hooks/useMessageQueue';
import type { RenderUnit } from '../../conversation/renderUnits';
import { buildSectionItems, lineTextAt as diffLineTextAt } from '../viewer/pierreDiffMapping';
import { findLiteralMatches, type ViewerFindMatch } from './literalMatch';

export interface SearchableSourceMatch<TTarget> {
  target: TTarget;
  start: number;
  end: number;
}

export interface SearchableSource<TTarget> {
  id: string;
  text: string;
  target: TTarget;
}

export interface SearchableSourceProjection<TTarget, TSource extends SearchableSource<TTarget>> {
  sources: TSource[];
  matches: SearchableSourceMatch<TTarget>[];
}

export interface FileSearchMatchTarget {
  kind: 'file-line';
  lineNumber: number;
  startColumn: number;
  endColumn: number;
}

export interface FileSearchSource extends SearchableSource<FileSearchMatchTarget> {
  kind: 'line';
  lineNumber: number;
}

export type FileSearchProjection = SearchableSourceProjection<FileSearchMatchTarget, FileSearchSource>;

export interface DiffSearchMatchTarget {
  kind: 'commit-log-line' | 'diff-file-header' | 'diff-line';
  section: DiffSection;
  filePath: string;
  itemId: string;
  side?: 'additions' | 'deletions';
  lineNumber?: number;
  startColumn: number;
  endColumn: number;
}

export interface DiffSearchSource extends SearchableSource<DiffSearchMatchTarget> {
  kind: 'commit-log' | 'file-header' | 'line';
  section: DiffSection;
  filePath: string;
  itemId: string;
  order: number;
  side?: 'additions' | 'deletions';
  lineNumber?: number;
}

export type DiffSearchProjection = SearchableSourceProjection<DiffSearchMatchTarget, DiffSearchSource>;

export interface ConversationUnitSearchMatchTarget {
  kind: 'unit-text';
  unitKey: string;
  unitKind: RenderUnit['kind'];
  unitIndex: number;
  sourceId: string;
  start: number;
  end: number;
}

export interface ConversationHeaderSearchMatchTarget {
  kind: 'header-text';
  headerKey: 'system-prompt';
  sourceId: string;
  start: number;
  end: number;
}

export type ConversationSearchMatchTarget =
  | ConversationUnitSearchMatchTarget
  | ConversationHeaderSearchMatchTarget;

export interface ConversationSearchSource extends SearchableSource<ConversationSearchMatchTarget> {
  kind: 'unit-text';
  unitKey: string;
  unitKind: RenderUnit['kind'];
  unitIndex: number;
  role: string;
}

export type ConversationSearchProjection = SearchableSourceProjection<
  ConversationSearchMatchTarget,
  ConversationSearchSource
>;

export interface BlockSearchMatchTarget {
  kind: 'block';
  blockId: string;
  lineNumber: number;
  startOffset: number;
  endOffset: number;
}

export interface BlockSearchSource extends SearchableSource<BlockSearchMatchTarget> {
  kind: 'block';
  blockId: string;
  lineNumber: number;
}

export type BlockSearchProjection = SearchableSourceProjection<BlockSearchMatchTarget, BlockSearchSource>;

export function buildFileSearchProjection(content: string, query: string): FileSearchProjection {
  const sources: FileSearchSource[] = [];
  let lineStart = 0;
  let lineNumber = 1;
  while (lineStart <= content.length) {
    const newline = content.indexOf('\n', lineStart);
    const end = newline === -1 ? content.length : newline;
    const text = content.slice(lineStart, end);
    sources.push({
      id: `line:${lineNumber}`,
      kind: 'line',
      lineNumber,
      text,
      target: { kind: 'file-line', lineNumber, startColumn: 0, endColumn: 0 },
    });
    if (newline === -1) break;
    lineStart = newline + 1;
    lineNumber += 1;
  }
  return { sources, matches: projectMatches(sources, query, (source, match) => ({
    kind: 'file-line',
    lineNumber: source.lineNumber,
    startColumn: match.start,
    endColumn: match.end,
  })) };
}

export function buildDiffSearchProjection(
  committedDiff: string | null | undefined,
  uncommittedDiff: string | null | undefined,
  query: string,
  commitLog: string | null | undefined = '',
): DiffSearchProjection {
  const sources: DiffSearchSource[] = [];
  let order = 0;
  const sections: Array<[DiffSection, string | null | undefined]> = [
    ['committed', committedDiff],
    ['uncommitted', uncommittedDiff],
  ];

  const commitLogLines = (commitLog ?? '').split('\n');
  for (let lineIndex = 0; lineIndex < commitLogLines.length; lineIndex += 1) {
    sources.push({
      id: `commit-log:${lineIndex}`,
      kind: 'commit-log',
      section: 'committed',
      filePath: '',
      itemId: `commit-log:${lineIndex}`,
      order: order++,
      text: commitLogLines[lineIndex] ?? '',
      target: {
        kind: 'commit-log-line',
        section: 'committed',
        filePath: '',
        itemId: `commit-log:${lineIndex}`,
        startColumn: 0,
        endColumn: 0,
      },
    });
  }

  for (const [section, rawDiff] of sections) {
    const built = buildSectionItems(section, rawDiff);
    for (const item of built.items) {
      const filePath = item.fileDiff.name;
      sources.push({
        id: `${item.id}:header`,
        kind: 'file-header',
        section,
        filePath,
        itemId: item.id,
        order: order++,
        text: headerText(item.fileDiff),
        target: {
          kind: 'diff-file-header',
          section,
          filePath,
          itemId: item.id,
          startColumn: 0,
          endColumn: 0,
        },
      });

      for (const source of buildDiffLineSources(item.fileDiff, item.id, section, filePath)) {
        sources.push({
          ...source,
          order: order++,
        });
      }
    }
  }

  return {
    sources,
    matches: projectMatches(sources, query, (source, match) => ({
      ...source.target,
      startColumn: match.start,
      endColumn: match.end,
    })),
  };
}

function makeDiffLineSource(
  itemId: string,
  section: DiffSection,
  filePath: string,
  order: number,
  side: 'additions' | 'deletions',
  lineNumber: number,
  text: string,
): DiffSearchSource {
  return {
    id: `${itemId}:${side}:${lineNumber}`,
    kind: 'line',
    section,
    filePath,
    itemId,
    order,
    side,
    lineNumber,
    text,
    target: {
      kind: 'diff-line',
      section,
      filePath,
      itemId,
      side,
      lineNumber,
      startColumn: 0,
      endColumn: 0,
    },
  };
}

function buildDiffLineSources(
  fileDiff: Parameters<typeof diffLineTextAt>[0],
  itemId: string,
  section: DiffSection,
  filePath: string,
): DiffSearchSource[] {
  const sources: DiffSearchSource[] = [];
  const additionLines = fileDiff.additionLines ?? [];
  const deletionLines = fileDiff.deletionLines ?? [];
  let additionCursor = 0;
  let deletionCursor = 0;

  for (const hunk of fileDiff.hunks) {
    let additionLine = hunk.additionStart;
    let deletionLine = hunk.deletionStart;

    for (const segment of hunk.hunkContent) {
      if (segment.type === 'context') {
        for (let offset = 0; offset < segment.lines; offset += 1) {
          const text = stripDiffSearchLineEnding(additionLines[additionCursor] ?? deletionLines[deletionCursor] ?? '');
          sources.push({
            id: `${itemId}:context:${additionLine}:${deletionLine}`,
            kind: 'line',
            section,
            filePath,
            itemId,
            order: 0,
            side: 'additions',
            lineNumber: additionLine,
            text,
            target: {
              kind: 'diff-line',
              section,
              filePath,
              itemId,
              side: 'additions',
              lineNumber: additionLine,
              startColumn: 0,
              endColumn: 0,
            },
          });
          additionCursor += 1;
          deletionCursor += 1;
          additionLine += 1;
          deletionLine += 1;
        }
        continue;
      }
      for (let offset = 0; offset < segment.deletions; offset += 1) {
        const text = stripDiffSearchLineEnding(deletionLines[deletionCursor] ?? '');
        sources.push(makeDiffLineSource(itemId, section, filePath, 0, 'deletions', deletionLine, text));
        deletionCursor += 1;
        deletionLine += 1;
      }
      for (let offset = 0; offset < segment.additions; offset += 1) {
        const text = stripDiffSearchLineEnding(additionLines[additionCursor] ?? '');
        sources.push(makeDiffLineSource(itemId, section, filePath, 0, 'additions', additionLine, text));
        additionCursor += 1;
        additionLine += 1;
      }
    }
  }

  return sources;
}

function stripDiffSearchLineEnding(text: string): string {
  return text.replace(/\r?\n$/, '');
}

function headerText(fileDiff: { name: string; prevName?: string }): string {
  return fileDiff.prevName && fileDiff.prevName !== '' ? `${fileDiff.prevName} → ${fileDiff.name}` : fileDiff.name;
}

export function buildBlockSearchProjection(
  blocks: readonly { id: string; lineNumber: number; text: string }[],
  query: string,
): BlockSearchProjection {
  const sources: BlockSearchSource[] = blocks
    .filter((block) => block.text.length > 0)
    .map((block) => ({
      id: block.id,
      kind: 'block',
      blockId: block.id,
      lineNumber: block.lineNumber,
      text: block.text,
      target: {
        kind: 'block',
        blockId: block.id,
        lineNumber: block.lineNumber,
        startOffset: 0,
        endOffset: 0,
      },
    }));

  return {
    sources,
    matches: projectMatches(sources, query, (source, match) => ({
      kind: 'block',
      blockId: source.blockId,
      lineNumber: source.lineNumber,
      startOffset: match.start,
      endOffset: match.end,
    })),
  };
}

export interface ConversationProjectionOptions {
  latestAgentKey?: string | null;
  density?: 'full' | 'compact';
  streamingBuffer?: StreamingBuffer | null;
  systemPrompt?: string | null;
  systemPromptExpanded?: boolean;
  liveBashProgress?: Readonly<Record<string, { progress: BashToolProgress }>>;
}

export function buildConversationSearchProjection(
  units: readonly RenderUnit[],
  query: string,
  options: ConversationProjectionOptions = {},
): ConversationSearchProjection {
  const density = options.density ?? 'full';
  const sources: ConversationSearchSource[] = [];
  if (options.systemPromptExpanded && options.systemPrompt) {
    addConversationHeaderSource(sources, 'system-prompt', options.systemPrompt);
  }

  units.forEach((unit, unitIndex) => {
    switch (unit.kind) {
      case 'user':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'user-message', userMessageText(unit.message));
        break;
      case 'pending_user':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'pending-user-message', queuedMessageText(unit.message));
        break;
      case 'skill':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'skill-message', skillMessageText(unit.message));
        break;
      case 'system':
        if (!isHiddenSystemMessage(unit.message)) {
          addConversationSource(sources, unitIndex, unit.kind, unit.key, 'system-message', userMessageText(unit.message));
        }
        break;
      case 'agent_turn':
        for (const source of agentTurnSources(unit.agent, unit.toolResultsByUseId, density, unit.key === options.latestAgentKey, options.liveBashProgress ?? {})) {
          addConversationSource(
            sources,
            unitIndex,
            unit.kind,
            unit.key,
            source.role,
            visibleConversationText(source.text),
          );
        }
        break;
      case 'streaming_agent':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'streaming-agent', visibleStreamingText(options.streamingBuffer));
        break;
      case 'sub_agent_status':
        addConversationSource(
          sources,
          unitIndex,
          unit.kind,
          unit.key,
          'sub-agent-status',
          [
            ...unit.state.completed_results.map((agent) => `completed ${agent.task} ${subAgentOutcomeText(agent.outcome)}`),
            ...unit.state.pending.map((agent) => `pending ${agent.task}`),
          ].join('\n'),
        );
        break;
      default:
        unit satisfies never;
    }
  });

  return {
    sources,
    matches: projectMatches(sources, query, (source, match) => source.target.kind === 'header-text'
      ? {
          kind: 'header-text',
          headerKey: source.target.headerKey,
          sourceId: source.id,
          start: match.start,
          end: match.end,
        }
      : {
          kind: 'unit-text',
          unitKey: source.unitKey,
          unitKind: source.unitKind,
          unitIndex: source.unitIndex,
          sourceId: source.id,
          start: match.start,
          end: match.end,
        }),
  };
}

function subAgentOutcomeText(outcome: { type: 'success'; result?: string } | { type: 'failure'; error?: string; error_kind?: string } | { type: 'timed_out' }): string {
  switch (outcome.type) {
    case 'success': return outcome.result ?? 'success';
    case 'failure': return outcome.error ?? outcome.error_kind ?? 'failure';
    case 'timed_out': return 'timed out';
  }
}

function addConversationHeaderSource(
  out: ConversationSearchSource[],
  headerKey: 'system-prompt',
  text: string,
): void {
  if (text.length === 0) return;
  out.push({
    id: `${headerKey}:${out.length}`,
    kind: 'unit-text',
    unitKey: `${headerKey}-header`,
    unitKind: 'system',
    unitIndex: -1,
    role: headerKey,
    text,
    target: {
      kind: 'header-text',
      headerKey,
      sourceId: `${headerKey}:${out.length}`,
      start: 0,
      end: 0,
    },
  });
}

function addConversationSource(
  out: ConversationSearchSource[],
  unitIndex: number,
  unitKind: RenderUnit['kind'],
  unitKey: string,
  role: string,
  text: string,
): void {
  if (text.length === 0) return;
  out.push({
    id: `${unitKey}:${role}:${out.length}`,
    kind: 'unit-text',
    unitKey,
    unitKind,
    unitIndex,
    role,
    text,
    target: {
      kind: 'unit-text',
      unitKey,
      unitKind,
      unitIndex,
      sourceId: `${unitKey}:${role}:${out.length}`,
      start: 0,
      end: 0,
    },
  });
}

function userMessageText(message: Message): string {
  if (typeof message.content === 'string') return message.content;
  const content = message.content as { text?: string; files?: Array<{ original_name?: string }> };
  const parts: string[] = [];
  if (typeof content.text === 'string' && content.text.length > 0) parts.push(content.text);
  for (const file of content.files ?? []) {
    if (typeof file.original_name === 'string' && file.original_name.length > 0) parts.push(file.original_name);
  }
  return parts.join('\n');
}

function skillMessageText(message: Message): string {
  const content = message.content as {
    text?: string;
    name?: string;
    trigger?: string;
    args?: string;
    source?: string;
    snippet?: string;
    files?: Array<{ original_name?: string }>;
  };
  const parts: string[] = [];
  const trigger = typeof content.trigger === 'string' && content.trigger.trim().length > 0
    ? content.trigger.trim()
    : [content.name ? `/${content.name}` : '', typeof content.args === 'string' ? content.args.trim() : '']
      .filter((part) => part.length > 0)
      .join(' ');
  if (trigger.length > 0) parts.push(trigger);
  for (const file of content.files ?? []) {
    if (typeof file.original_name === 'string' && file.original_name.length > 0) parts.push(file.original_name);
  }
  return parts.join('\n');
}

function queuedMessageText(message: QueuedMessage): string {
  const parts = [message.text, ...(message.files ?? []).map((file) => file.original_name)];
  return parts.filter((part) => part.length > 0).join('\n');
}

function isHiddenSystemMessage(message: Message): boolean {
  const displayData = message.display_data as { hidden?: boolean } | null | undefined;
  return displayData?.hidden === true;
}

function visibleConversationText(text: string): string {
  return text;
}

function visibleStreamingText(buffer: StreamingBuffer | null | undefined): string {
  return buffer?.text ?? '';
}

function toolResultText(result: Message | undefined): string {
  if (!result) return '';
  const content = result.content as ToolResultContent | undefined;
  return content?.content || content?.result || content?.error || '';
}

function tryParseJsonText(text: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(text);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, unknown> : null;
  } catch {
    return null;
  }
}

function formatVisibleMillis(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(1)}s`;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  const remaining = Math.round(seconds % 60);
  return remaining > 0 ? `${minutes}m ${remaining}s` : `${minutes}m`;
}

function compactBashTailText(parts: string[]): string {
  const text = parts.join(' · ');
  return text.length > 140 ? `${text.slice(0, 139)}…` : text;
}

function bashVisibleSearchText(
  block: ContentBlock,
  result: Message | undefined,
  progress: BashToolProgress | undefined,
  density: 'full' | 'compact',
): string {
  const parts: string[] = [];
  if (block.display) parts.push(block.display);
  const input = block.input as Record<string, unknown> | undefined;
  const op = typeof input?.['op'] === 'string' ? input['op'] : null;
  const handle = typeof input?.['handle'] === 'string' ? input['handle'] : null;
  if (op === 'wait' && handle) parts.push(`wait ${handle}`);
  if (op === 'peek' && handle) parts.push(`peek ${handle}`);
  if (op === 'kill' && handle) parts.push(`kill ${handle}`);
  const parsed = result ? tryParseJsonText(toolResultText(result)) : null;
  if (parsed) {
    if (typeof parsed['status'] === 'string') {
      const status = String(parsed['status']);
      const finalCause = typeof parsed['final_cause'] === 'string' ? parsed['final_cause'] : null;
      parts.push(status === 'kill_pending_kernel'
        ? 'kill pending (kernel)'
        : status === 'tombstoned' && finalCause
          ? `tombstoned ${finalCause.replace(/_/g, ' ')}`
          : status.replace(/_/g, ' '));
    }
    if (typeof parsed['handle'] === 'string') parts.push(parsed['handle'] as string);
    if (typeof parsed['waited_ms'] === 'number') parts.push(`waited ${formatVisibleMillis(parsed['waited_ms'])}`);
    if (typeof parsed['duration_ms'] === 'number') parts.push(`ran ${formatVisibleMillis(parsed['duration_ms'])}`);
    if (parsed['exit_code'] !== undefined && parsed['exit_code'] !== null) parts.push(`exit ${String(parsed['exit_code'])}`);
    if (typeof parsed['error'] === 'string') parts.push(parsed['error']);
    if (typeof parsed['error_message'] === 'string') parts.push(parsed['error_message']);
    if (parsed['truncated_before'] === true) parts.push('older output omitted');
    const lines = Array.isArray(parsed['lines']) ? parsed['lines'] : [];
    const visibleLines = density === 'full' ? lines : lines.slice(-2);
    const outputParts: string[] = [];
    for (const line of visibleLines) {
      const text = line && typeof line === 'object' && typeof (line as { bytes?: unknown }).bytes === 'string'
        ? (line as { bytes: string }).bytes
        : '';
      if (text) outputParts.push(text);
    }
    if (typeof parsed['partial'] === 'string' && parsed['partial'].length > 0) outputParts.push(parsed['partial']);
    parts.push(density === 'compact' ? compactBashTailText(outputParts) : outputParts.join('\n'));
    return parts.join('\n');
  }
  if (result) parts.push(toolResultText(result));
  if (progress) {
    if (progress.truncated_before) parts.push('older output omitted');
    const visibleProgressLines = density === 'full' ? progress.lines.slice(-8) : progress.lines.slice(-2);
    const progressParts = visibleProgressLines.map((line) => line.text);
    if (progress.partial) progressParts.push(progress.partial);
    parts.push(density === 'compact' ? compactBashTailText(progressParts) : progressParts.join('\n'));
  }
  return parts.join('\n');
}

function agentTurnSources(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
  density: 'full' | 'compact',
  isLatestAgentMessage: boolean,
  liveBashProgress: Readonly<Record<string, { progress: BashToolProgress }>>,
): Array<{ role: string; text: string; forceExpanded?: boolean }> {
  const forceExpandedText = isLatestAgentMessage
    || (message.display_data as { forceExpandedText?: boolean } | null | undefined)?.forceExpandedText === true;
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const out: Array<{ role: string; text: string; forceExpanded?: boolean }> = [];
  blocks.forEach((block, index) => {
    if (block.type === 'text') {
      out.push({ role: `agent-text-${index}`, text: block.text ?? '', forceExpanded: forceExpandedText });
      return;
    }
    if (block.type === 'tool_use') {
      out.push({ role: `tool-use-name-${index}`, text: block.name ?? '' });
      out.push({ role: `tool-use-display-${index}`, text: block.display ?? '' });
      const result = toolResultsByUseId.get(block.id ?? '');
      if (block.name === 'bash') {
        out.push({ role: `tool-use-visible-bash-${index}`, text: bashVisibleSearchText(block, result, liveBashProgress[block.id ?? '']?.progress, density) });
        if (density === 'full') out.push({ role: `tool-use-input-${index}`, text: stableJson(block.input) });
      } else if (densityToolDetailsVisible(block.name, density)) {
        out.push({ role: `tool-use-input-${index}`, text: stableJson(block.input) });
        out.push({ role: `tool-use-result-${index}`, text: toolResultText(result) });
      }
      return;
    }
  });
  return out;
}

function densityToolDetailsVisible(toolName: string | undefined, density: 'full' | 'compact'): boolean {
  if (toolName === 'think') return true;
  return density === 'full';
}

function stableJson(value: unknown): string {
  if (value === undefined) return '';
  return JSON.stringify(sortJsonValue(value), null, 2) ?? '';
}

function sortJsonValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJsonValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, child]) => [key, sortJsonValue(child)]),
    );
  }
  return value;
}

function projectMatches<TTarget, TSource extends SearchableSource<TTarget>>(
  sources: readonly TSource[],
  query: string,
  buildTarget: (source: TSource, match: ViewerFindMatch) => TTarget,
): SearchableSourceMatch<TTarget>[] {
  const out: SearchableSourceMatch<TTarget>[] = [];
  for (const source of sources) {
    for (const match of findLiteralMatches(source.text, query).matches) {
      out.push({ target: buildTarget(source, match), start: match.start, end: match.end });
    }
  }
  return out;
}
