import type { ContentBlock, Message, ToolResultContent } from '../../api';
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
  kind: 'diff-file-header' | 'diff-line';
  section: DiffSection;
  filePath: string;
  itemId: string;
  side?: 'additions' | 'deletions';
  lineNumber?: number;
  startColumn: number;
  endColumn: number;
}

export interface DiffSearchSource extends SearchableSource<DiffSearchMatchTarget> {
  kind: 'file-header' | 'line';
  section: DiffSection;
  filePath: string;
  itemId: string;
  order: number;
  side?: 'additions' | 'deletions';
  lineNumber?: number;
}

export type DiffSearchProjection = SearchableSourceProjection<DiffSearchMatchTarget, DiffSearchSource>;

export interface ConversationSearchMatchTarget {
  kind: 'unit-text';
  unitKey: string;
  unitKind: RenderUnit['kind'];
  unitIndex: number;
  sourceId: string;
  start: number;
  end: number;
}

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
): DiffSearchProjection {
  const sources: DiffSearchSource[] = [];
  let order = 0;
  const sections: Array<[DiffSection, string | null | undefined]> = [
    ['committed', committedDiff],
    ['uncommitted', uncommittedDiff],
  ];

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

      for (const hunk of item.fileDiff.hunks) {
        const seenAdditionLines = new Set<number>();
        const seenDeletionLines = new Set<number>();
        const maxRows = Math.max(hunk.additionCount, hunk.deletionCount);
        for (let offset = 0; offset < maxRows; offset++) {
          const additionLine = hunk.additionStart + offset;
          const deletionLine = hunk.deletionStart + offset;
          const additionText = diffLineTextAt(item.fileDiff, 'additions', additionLine);
          const deletionText = diffLineTextAt(item.fileDiff, 'deletions', deletionLine);
          if (
            additionText !== undefined
            && deletionText !== undefined
            && additionText === deletionText
            && !seenAdditionLines.has(additionLine)
            && !seenDeletionLines.has(deletionLine)
          ) {
            seenAdditionLines.add(additionLine);
            seenDeletionLines.add(deletionLine);
            sources.push({
              id: `${item.id}:context:${additionLine}:${deletionLine}`,
              kind: 'line',
              section,
              filePath,
              itemId: item.id,
              order: order++,
              side: 'additions',
              lineNumber: additionLine,
              text: additionText,
              target: {
                kind: 'diff-line',
                section,
                filePath,
                itemId: item.id,
                side: 'additions',
                lineNumber: additionLine,
                startColumn: 0,
                endColumn: 0,
              },
            });
            continue;
          }
          if (deletionText !== undefined && !seenDeletionLines.has(deletionLine)) {
            seenDeletionLines.add(deletionLine);
            sources.push(makeDiffLineSource(item.id, section, filePath, order++, 'deletions', deletionLine, deletionText));
          }
          if (additionText !== undefined && !seenAdditionLines.has(additionLine)) {
            seenAdditionLines.add(additionLine);
            sources.push(makeDiffLineSource(item.id, section, filePath, order++, 'additions', additionLine, additionText));
          }
        }
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
  density?: 'full' | 'compact';
  streamingBuffer?: StreamingBuffer | null;
}

export function buildConversationSearchProjection(
  units: readonly RenderUnit[],
  query: string,
  options: ConversationProjectionOptions = {},
): ConversationSearchProjection {
  const density = options.density ?? 'full';
  const sources: ConversationSearchSource[] = [];
  units.forEach((unit, unitIndex) => {
    switch (unit.kind) {
      case 'user':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'user-message', userMessageText(unit.message));
        break;
      case 'pending_user':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'pending-user-message', queuedMessageText(unit.message));
        break;
      case 'skill':
        addConversationSource(sources, unitIndex, unit.kind, unit.key, 'skill-message', userMessageText(unit.message));
        break;
      case 'system':
        if (!isHiddenSystemMessage(unit.message)) {
          addConversationSource(sources, unitIndex, unit.kind, unit.key, 'system-message', userMessageText(unit.message));
        }
        break;
      case 'agent_turn':
        for (const source of agentTurnSources(unit.agent, unit.toolResultsByUseId)) {
          addConversationSource(sources, unitIndex, unit.kind, unit.key, source.role, visibleConversationText(source.text, density));
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
            ...unit.state.pending.map((agent) => `pending ${agent.task}`),
            ...unit.state.completed_results.map((agent) => `completed ${agent.task} ${subAgentOutcomeText(agent.outcome)}`),
          ].join('\n'),
        );
        break;
      default:
        unit satisfies never;
    }
  });

  return {
    sources,
    matches: projectMatches(sources, query, (source, match) => ({
      kind: 'unit-text',
      unitKey: source.unitKey,
      unitKind: source.unitKind,
      unitIndex: source.unitIndex,
      sourceId: source.id,
      start: match.start,
      end: match.end,
    })),
  };
}

function subAgentOutcomeText(outcome: { type: 'success'; result?: string } | { type: 'failure'; error?: string; error_kind?: string } | { type: 'timed_out' }): string {
  switch (outcome.type) {
    case 'success': return outcome.result ?? 'success';
    case 'failure': return outcome.error ?? outcome.error_kind ?? 'failure';
    case 'timed_out': return 'timed out';
  }
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
  const content = message.content as { text?: string; files?: Array<{ original_name?: string }> };
  const parts: string[] = [];
  if (typeof content.text === 'string' && content.text.length > 0) parts.push(content.text);
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

function visibleConversationText(text: string, density: 'full' | 'compact'): string {
  if (density !== 'compact' || !shouldCollapseCompactText(text)) return text;
  return firstLineSummary(text);
}

function visibleStreamingText(buffer: StreamingBuffer | null | undefined): string {
  return buffer?.text ?? '';
}

function firstLineSummary(text: string, maxLen = 140): string {
  const firstLine = text.split('\n').find((l) => l.trim()) ?? text;
  const flat = firstLine.replace(/\s+/g, ' ').trim();
  return flat.length > maxLen ? `${flat.slice(0, maxLen - 1)}…` : flat;
}

function shouldCollapseCompactText(text: string): boolean {
  const nonEmptyLines = text.split('\n').filter((line) => line.trim());
  const firstLineFlat = (nonEmptyLines[0] ?? '').replace(/\s+/g, ' ').trim();
  const fullFlat = text.replace(/\s+/g, ' ').trim();
  const significantThreshold = 280;
  if (text.length >= significantThreshold) return false;
  const hidesAdditionalLines = nonEmptyLines.length > 1 && firstLineFlat !== fullFlat;
  const truncatesFirstLine = firstLineFlat.length > 140;
  return hidesAdditionalLines || truncatesFirstLine;
}

function toolResultText(result: Message | undefined): string {
  if (!result) return '';
  const content = result.content as ToolResultContent | undefined;
  return content?.content || content?.result || content?.error || '';
}

function agentTurnSources(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
): Array<{ role: string; text: string }> {
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const out: Array<{ role: string; text: string }> = [];
  blocks.forEach((block, index) => {
    if (block.type === 'text') {
      out.push({ role: `agent-text-${index}`, text: block.text ?? '' });
      return;
    }
    if (block.type === 'tool_use') {
      out.push({ role: `tool-use-name-${index}`, text: block.name ?? '' });
      out.push({ role: `tool-use-display-${index}`, text: block.display ?? '' });
      out.push({ role: `tool-use-input-${index}`, text: stableJson(block.input) });
      out.push({ role: `tool-use-result-${index}`, text: toolResultText(toolResultsByUseId.get(block.id ?? '')) });
      return;
    }
  });
  return out;
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
