import { toString as mdastToString } from 'mdast-util-to-string';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import { unified } from 'unified';

import type { ContentBlock, Message, ToolResultContent } from '../../api';
import type { StreamingBuffer } from '../../conversation/atom';
import type { DiffSection } from '../../contexts/ReviewNotesContext';
import type { QueuedMessage } from '../../hooks/useMessageQueue';
import type { RenderUnit } from '../../conversation/renderUnits';
import { buildSectionItems, lineTextAt as diffLineTextAt } from '../viewer/pierreDiffMapping';
import { findLiteralMatches, type ViewerFindMatch } from './literalMatch';

export interface SearchableSourceMatch<TTarget> {
  sourceId: string;
  sourceText: string;
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
  matchOrdinal?: number;
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
  fragmentId?: string;
  start: number;
  end: number;
}

export interface ConversationTextFragmentDisplay {
  mode: 'full' | 'compact-collapsed';
  summaryText: string;
}

export interface SearchResultRevealTarget {
  kind: 'tool-result-search';
  key: string;
  path?: string;
  lineNumber?: number;
}

export interface ReadFileLineFragmentDisplay {
  lineNumber: number;
  content: string;
}

export interface ReadFilePathFragmentDisplay {
  path: string;
  windowLabel?: string;
}

export interface ReadFileRevealTarget {
  kind: 'tool-result-read-file';
  toolUseId: string;
  fragmentId: string;
  lineNumber?: number;
  startLineNumber?: number;
  endLineNumber?: number;
}

export type ReadFileFragment =
  | {
      fragmentId: string;
      semanticText: string;
      display: ReadFilePathFragmentDisplay;
      revealTarget: ReadFileRevealTarget;
      kind: 'path';
    }
  | {
      fragmentId: string;
      semanticText: string;
      display: ReadFileLineFragmentDisplay;
      revealTarget: ReadFileRevealTarget;
      kind: 'line';
    };

export interface ReadFileOutputProjection {
  fragments: ReadonlyArray<ReadFileFragment>;
  fullText: string;
}

export interface SubAgentCardRevealTarget {
  kind: 'subagent-card';
  toolUseId: string;
  agentId: string;
  fragmentId: string;
}

export interface SubAgentCardFragment {
  fragmentId: string;
  semanticText: string;
  display: { agentId: string; task: string; outcomeText: string };
  revealTarget: SubAgentCardRevealTarget;
  kind: 'subagent-card';
}

export type TerminalToolResultFamily = 'bash' | 'tmux' | 'browser-profile' | 'opaque';

export interface TerminalToolResultRevealTarget {
  kind: 'tool-result-terminal';
  toolUseId: string;
  fragmentId: string;
  family: TerminalToolResultFamily;
}

export interface TerminalToolResultFragment {
  fragmentId: string;
  semanticText: string;
  display: { family: TerminalToolResultFamily };
  revealTarget: TerminalToolResultRevealTarget;
  kind: 'terminal-result';
}

export interface PatchRevealTarget {
  kind: 'tool-result-patch';
  toolUseId: string;
  fragmentId: string;
}

export interface PatchFragment {
  fragmentId: string;
  semanticText: string;
  display: { diff: string };
  revealTarget: PatchRevealTarget;
  kind: 'diff';
}

export interface PatchOutputProjection {
  fragments: readonly [PatchFragment];
  fullText: string;
}

export interface KeywordSearchRevealTarget {
  kind: 'tool-result-keyword-search';
  key: string;
}

export interface ConversationTextFragmentRevealTarget {
  kind: 'agent-text';
  key: string;
}

export type ConversationFragmentRevealTarget =
  | ConversationTextFragmentRevealTarget
  | SearchResultRevealTarget
  | KeywordSearchRevealTarget
  | ReadFileRevealTarget
  | PatchRevealTarget
  | TerminalToolResultRevealTarget
  | SubAgentCardRevealTarget;

export interface ConversationTextFragment {
  fragmentId: string;
  semanticText: string;
  display: ConversationTextFragmentDisplay;
  revealTarget: ConversationFragmentRevealTarget;
}

export interface KeywordSearchFragmentDisplay {
  title: string;
  body?: string;
}

export interface KeywordSearchFragment {
  fragmentId: string;
  semanticText: string;
  display: KeywordSearchFragmentDisplay;
  revealTarget: ConversationFragmentRevealTarget;
  kind: 'hit' | 'fallback' | 'empty';
  path?: string;
  explanation?: string;
}

export interface KeywordSearchOutputProjection {
  hits: Array<{ path: string; explanation: string; fragment: KeywordSearchFragment }>;
  rawFallback: boolean;
  empty: boolean;
  fallbackText: string | null;
  fragments: KeywordSearchFragment[];
}

export interface SearchResultFragmentDisplay {
  path: string;
  lineNumber?: number;
  content?: string;
  note?: string;
}

export interface SearchResultFragment {
  fragmentId: string;
  semanticText: string;
  display: SearchResultFragmentDisplay;
  revealTarget: SearchResultRevealTarget;
  kind: 'hit' | 'note' | 'empty' | 'fallback';
}

export interface SearchHitProjection {
  path: string;
  lineNumber: number;
  content: string;
  fragment: SearchResultFragment;
}

export interface SearchGroupProjection {
  path: string;
  hits: SearchHitProjection[];
}

export interface SearchOutputProjection {
  hits: SearchHitProjection[];
  groups: SearchGroupProjection[];
  notes: Array<{ text: string; fragment: SearchResultFragment }>;
  noMatches: boolean;
  rawFallback: boolean;
  fallbackText: string | null;
  fragments: SearchResultFragment[];
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
  fragmentId?: string;
  revealTarget?: ConversationFragmentRevealTarget;
}

export type ConversationSearchProjection = SearchableSourceProjection<
  ConversationSearchMatchTarget,
  ConversationSearchSource
>;

interface MarkdownNodePosition {
  start?: { line?: number; offset?: number };
  end?: { line?: number; offset?: number };
}

interface MarkdownNode {
  type: string;
  position?: MarkdownNodePosition;
  children?: MarkdownNode[];
  value?: string;
  alt?: string;
}

export interface MarkdownDisplayBlock {
  id: string;
  lineNumber: number;
  sourceRange: { start: number; end: number };
  searchableText: string;
  kind: string;
}

export function buildMarkdownDisplayBlocks(markdown: string): readonly MarkdownDisplayBlock[] {
  const processor = unified().use(remarkParse).use(remarkGfm);
  const tree = processor.runSync(processor.parse(markdown)) as MarkdownNode;
  const blocks: MarkdownDisplayBlock[] = [];
  const visit = (node: MarkdownNode, path: readonly number[]) => {
    if (node.type !== 'root' && node.position?.start?.line && node.position.start.offset !== undefined && node.position.end?.offset !== undefined) {
      const isDisplayBlock = ['paragraph', 'heading', 'tableCell', 'code'].includes(node.type);
      if (isDisplayBlock) {
        const searchableText = mdastToString(node as Parameters<typeof mdastToString>[0]);
        if (searchableText) {
          blocks.push({
            id: `markdown:${path.join('.')}:${node.position.start.offset}-${node.position.end.offset}`,
            lineNumber: node.position.start.line,
            sourceRange: { start: node.position.start.offset, end: node.position.end.offset },
            searchableText,
            kind: node.type,
          });
        }
      }
    }
    node.children?.forEach((child, index) => visit(child, [...path, index]));
  };
  visit(tree, []);
  return blocks;
}

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
  return { sources, matches: projectMatches(sources, query, (source, match, matchOrdinal) => ({
    kind: 'file-line',
    lineNumber: source.lineNumber,
    startColumn: match.start,
    endColumn: match.end,
    matchOrdinal,
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
        for (const source of agentTurnSources(unit.agent, unit.toolResultsByUseId, density, unit.key === options.latestAgentKey)) {
          addConversationSource(
            sources,
            unitIndex,
            unit.kind,
            unit.key,
            source.role,
            source.text,
            source.fragmentId,
            source.revealTarget,
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
          ...(source.fragmentId ? { fragmentId: source.fragmentId } : {}),
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
    id: `conversation-header:${headerKey}`,
    kind: 'unit-text',
    unitKey: `${headerKey}-header`,
    unitKind: 'system',
    unitIndex: -1,
    role: headerKey,
    text,
    target: {
      kind: 'header-text',
      headerKey,
      sourceId: `conversation-header:${headerKey}`,
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
  fragmentId?: string,
  revealTarget?: ConversationFragmentRevealTarget,
): void {
  if (text.length === 0) return;
  out.push({
    id: `${unitKey}:${role}`,
    kind: 'unit-text',
    unitKey,
    unitKind,
    unitIndex,
    role,
    ...(fragmentId ? { fragmentId } : {}),
    ...(revealTarget ? { revealTarget } : {}),
    text,
    target: {
      kind: 'unit-text',
      unitKey,
      unitKind,
      unitIndex,
      sourceId: `${unitKey}:${role}`,
      ...(fragmentId ? { fragmentId } : {}),
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

function visibleStreamingText(buffer: StreamingBuffer | null | undefined): string {
  return buffer?.text ?? '';
}

function firstLineSummary(text: string, maxLen = 140): string {
  const firstLine = text.split('\n').find((l) => l.trim()) ?? text;
  const flat = firstLine.replace(/\s+/g, ' ').trim();
  return flat.length > maxLen ? `${flat.slice(0, maxLen - 1)}…` : flat;
}

function shouldCollapseCompactText(text: string): boolean {
  if (containsMermaidFence(text)) return false;
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

function keywordSearchToolResultKey(blockId: string): string {
  return `keyword-search:${blockId}`;
}

function searchToolResultKey(blockId: string): string {
  return `search:${blockId}`;
}

function readFileWindowLabel(input: Record<string, unknown>): string | null {
  const offset = typeof input['offset'] === 'number' ? input['offset'] : undefined;
  const limit = typeof input['limit'] === 'number' ? input['limit'] : undefined;
  if (offset === undefined && limit === undefined) return null;
  const start = offset ?? 1;
  if (limit === undefined) return `${start}+`;
  const end = start + Math.max(limit, 0) - 1;
  return `${start}-${end}`;
}

function parseReadFileRenderedLine(line: string): { lineNumber: number; content: string } | null {
  const match = /^(\s*)(\d+)\t(.*)$/.exec(line);
  if (!match?.[2]) return null;
  return {
    lineNumber: Number.parseInt(match[2], 10),
    content: match[3] ?? '',
  };
}

function buildReadFileProjectionFragments(
  text: string,
  input: Record<string, unknown>,
  options: { toolUseId?: string | null } = {},
): ReadFileFragment[] {
  const path = typeof input['path'] === 'string' ? input['path'] : '';
  const revealBase = {
    kind: 'tool-result-read-file' as const,
    toolUseId: options.toolUseId ?? '',
  };
  const fragments: ReadFileFragment[] = [];
  const windowLabel = readFileWindowLabel(input);
  if (path.length > 0) {
    const pathText = windowLabel ? `${path}:${windowLabel}` : path;
    fragments.push({
      fragmentId: 'read-file-path',
      semanticText: pathText,
      display: { path, ...(windowLabel ? { windowLabel } : {}) },
      revealTarget: {
        ...revealBase,
        fragmentId: 'read-file-path',
      },
      kind: 'path',
    });
  }

  const duplicateLineCounts = new Map<string, number>();
  const startLine = typeof input['offset'] === 'number' ? input['offset'] : 1;
  for (const [lineIndex, rawLine] of text.split('\n').entries()) {
    const renderedLine = parseReadFileRenderedLine(rawLine) ?? {
      lineNumber: startLine + lineIndex,
      content: rawLine,
    };
    const duplicateIndex = duplicateLineCounts.get(renderedLine.content) ?? 0;
    duplicateLineCounts.set(renderedLine.content, duplicateIndex + 1);
    const fragmentId = `read-file-line:${encodeURIComponent(renderedLine.content)}:${duplicateIndex}`;
    fragments.push({
      fragmentId,
      semanticText: `${renderedLine.lineNumber}\t${renderedLine.content}`,
      display: renderedLine,
      revealTarget: {
        ...revealBase,
        fragmentId,
        lineNumber: renderedLine.lineNumber,
        startLineNumber: renderedLine.lineNumber,
        endLineNumber: renderedLine.lineNumber,
      },
      kind: 'line',
    });
  }
  return fragments;
}

export function buildReadFileOutputProjection(
  text: string,
  input: Record<string, unknown>,
  options: { toolUseId?: string | null } = {},
): ReadFileOutputProjection {
  const fragments = buildReadFileProjectionFragments(text, input, options);
  const fullText = fragments.map((fragment) => fragment.semanticText).join('\n');
  return { fragments, fullText };
}

export function buildSubAgentCardFragments(displayData: unknown, toolUseId = ''): readonly SubAgentCardFragment[] {
  if (!displayData || typeof displayData !== 'object') return [];
  const data = displayData as Record<string, unknown>;
  if (data['type'] !== 'subagent_summary' || !Array.isArray(data['results'])) return [];
  return data['results'].flatMap((entry) => {
    if (!entry || typeof entry !== 'object') return [];
    const result = entry as Record<string, unknown>;
    const agentId = typeof result['agent_id'] === 'string' ? result['agent_id'] : '';
    const task = typeof result['task'] === 'string' ? result['task'] : '';
    const outcomeText = semanticSubAgentOutcome(result['outcome']);
    if (!agentId || (!task && !outcomeText)) return [];
    const fragmentId = `subagent-card:${agentId}`;
    return [{
      fragmentId,
      semanticText: [task, outcomeText].filter(Boolean).join('\n'),
      display: { agentId, task, outcomeText },
      revealTarget: { kind: 'subagent-card' as const, toolUseId, agentId, fragmentId },
      kind: 'subagent-card' as const,
    }];
  });
}

function semanticSubAgentOutcome(outcome: unknown): string {
  if (!outcome || typeof outcome !== 'object') return '';
  const value = outcome as Record<string, unknown>;
  for (const key of ['result', 'error', 'partial_result']) {
    if (typeof value[key] === 'string') return value[key];
  }
  return '';
}

export function buildTerminalToolResultProjection(
  family: TerminalToolResultFamily,
  resultText: string,
  displayData: unknown,
  options: { toolUseId?: string | null } = {},
): { fragments: readonly [TerminalToolResultFragment]; fullText: string } {
  const semanticText = family === 'browser-profile' && displayData
    ? semanticObjectText(displayData)
    : family === 'bash' || family === 'tmux'
      ? semanticStructuredResultText(resultText)
      : resultText;
  const fragmentId = `terminal-result:${family}`;
  const fragment: TerminalToolResultFragment = {
    fragmentId,
    semanticText,
    display: { family },
    revealTarget: {
      kind: 'tool-result-terminal',
      toolUseId: options.toolUseId ?? '',
      fragmentId,
      family,
    },
    kind: 'terminal-result',
  };
  return { fragments: [fragment], fullText: semanticText };
}

function semanticStructuredResultText(resultText: string): string {
  try {
    return semanticObjectText(JSON.parse(resultText));
  } catch {
    return resultText;
  }
}

function semanticObjectText(value: unknown, label?: string): string {
  if (value === null || value === undefined) return label ? `${label}: ${String(value)}` : String(value);
  if (Array.isArray(value)) {
    return value.map((entry, index) => semanticObjectText(entry, label ? `${label} ${index + 1}` : String(index + 1))).join('\n');
  }
  if (typeof value === 'object') {
    return Object.entries(value as Record<string, unknown>)
      .map(([key, entry]) => semanticObjectText(entry, key.replaceAll('_', ' ')))
      .filter(Boolean)
      .join('\n');
  }
  return label ? `${label}: ${String(value)}` : String(value);
}

export function buildPatchOutputProjection(
  diff: string,
  options: { toolUseId?: string | null } = {},
): PatchOutputProjection {
  const fragmentId = 'patch-diff';
  const fragment: PatchFragment = {
    fragmentId,
    semanticText: diff,
    display: { diff },
    revealTarget: {
      kind: 'tool-result-patch',
      toolUseId: options.toolUseId ?? '',
      fragmentId,
    },
    kind: 'diff',
  };
  return { fragments: [fragment], fullText: diff };
}

export function buildSearchOutputProjection(
  text: string,
  options: { toolUseId?: string | null } = {},
): SearchOutputProjection {
  const revealBase = {
    kind: 'tool-result-search' as const,
    key: searchToolResultKey(options.toolUseId ?? ''),
  };
  const trimmed = text.trim();
  if (trimmed === 'No matches found.') {
    const fragment: SearchResultFragment = {
      fragmentId: 'search-empty',
      semanticText: 'No matches found.',
      display: { path: '', note: 'No matches found.' },
      revealTarget: revealBase,
      kind: 'empty',
    };
    return { hits: [], groups: [], notes: [], noMatches: true, rawFallback: false, fallbackText: null, fragments: [fragment] };
  }

  const notes: Array<{ text: string; fragment: SearchResultFragment }> = [];
  const hits: SearchHitProjection[] = [];
  const duplicateCounts = new Map<string, number>();
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    if (line.startsWith('[') && line.trimEnd().endsWith(']')) {
      const note = line.trim().slice(1, -1);
      const duplicateIndex = duplicateCounts.get(note) ?? 0;
      duplicateCounts.set(note, duplicateIndex + 1);
      const fragment: SearchResultFragment = {
        fragmentId: `search-note:${encodeURIComponent(note)}:${duplicateIndex}`,
        semanticText: note,
        display: { path: '', note },
        revealTarget: revealBase,
        kind: 'note',
      };
      notes.push({ text: note, fragment });
      continue;
    }
    const m = /^(.+?):(\d+):\s?(.*)$/.exec(line);
    if (m && m[1] !== undefined && m[2] !== undefined) {
      const path = m[1];
      const lineNumber = parseInt(m[2], 10);
      const content = m[3] ?? '';
      const semanticText = `${path}:${lineNumber}: ${content}`;
      const duplicateIndex = duplicateCounts.get(semanticText) ?? 0;
      duplicateCounts.set(semanticText, duplicateIndex + 1);
      const fragment: SearchResultFragment = {
        fragmentId: `search-hit:${encodeURIComponent(semanticText)}:${duplicateIndex}`,
        semanticText,
        display: { path, lineNumber, content },
        revealTarget: { ...revealBase, path, lineNumber },
        kind: 'hit',
      };
      hits.push({ path, lineNumber, content, fragment });
      continue;
    }
    const note = line;
    const duplicateIndex = duplicateCounts.get(note) ?? 0;
    duplicateCounts.set(note, duplicateIndex + 1);
    const fragment: SearchResultFragment = {
      fragmentId: `search-note:${encodeURIComponent(note)}:${duplicateIndex}`,
      semanticText: note,
      display: { path: '', note },
      revealTarget: revealBase,
      kind: 'note',
    };
    notes.push({ text: note, fragment });
  }

  if (hits.length === 0 && notes.length === 0) {
    const fragment: SearchResultFragment = {
      fragmentId: 'search-fallback',
      semanticText: text,
      display: { path: '', note: text },
      revealTarget: revealBase,
      kind: 'fallback',
    };
    return { hits: [], groups: [], notes: [], noMatches: false, rawFallback: true, fallbackText: text, fragments: [fragment] };
  }

  const groups: SearchGroupProjection[] = [];
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

  return {
    hits,
    groups,
    notes,
    noMatches: false,
    rawFallback: false,
    fallbackText: null,
    fragments: [
      ...hits.map((hit) => hit.fragment),
      ...notes.map((note) => note.fragment),
    ],
  };
}

export function buildKeywordSearchOutputProjection(
  text: string,
  options: { toolUseId?: string | null } = {},
): KeywordSearchOutputProjection {
  const trimmed = text.trim();
  const revealTarget = {
    kind: 'tool-result-keyword-search' as const,
    key: keywordSearchToolResultKey(options.toolUseId ?? ''),
  };
  if (
    trimmed === '' ||
    trimmed === 'No matches found for the given search terms.' ||
    trimmed.startsWith('No relevant files found')
  ) {
    const fragment: KeywordSearchFragment = {
      fragmentId: 'keyword-search-empty',
      semanticText: 'No relevant files found.',
      display: { title: 'No relevant files found.' },
      revealTarget,
      kind: 'empty',
    };
    return { hits: [], rawFallback: false, empty: true, fallbackText: null, fragments: [fragment] };
  }

  const lines = text.split('\n').filter((l) => l.trim());
  const ripgrepShaped = lines.filter((l) => /^[^\s].*?[-:]\d+[-:]/.test(l) || l === '--').length;
  if (lines.length >= 4 && ripgrepShaped / lines.length > 0.25) {
    const fragment: KeywordSearchFragment = {
      fragmentId: 'keyword-search-fallback',
      semanticText: text,
      display: { title: 'Raw ripgrep results — LLM filter unavailable', body: text },
      revealTarget,
      kind: 'fallback',
    };
    return { hits: [], rawFallback: true, empty: false, fallbackText: text, fragments: [fragment] };
  }

  const hits: Array<{ path: string; explanation: string; fragment: KeywordSearchFragment }> = [];
  const duplicateCounts = new Map<string, number>();
  for (const line of lines) {
    const m = /^([^:\s][^:]*?):\s+(.+)$/.exec(line);
    if (m && m[1] !== undefined && m[2] !== undefined) {
      const path = m[1].trim();
      const explanation = m[2].trim();
      const semanticText = `${path}: ${explanation}`;
      const duplicateIndex = duplicateCounts.get(semanticText) ?? 0;
      duplicateCounts.set(semanticText, duplicateIndex + 1);
      hits.push({
        path,
        explanation,
        fragment: {
          fragmentId: `keyword-search-hit:${encodeURIComponent(semanticText)}:${duplicateIndex}`,
          semanticText,
          display: { title: path, body: explanation },
          revealTarget,
          kind: 'hit',
          path,
          explanation,
        },
      });
    }
  }

  if (hits.length === 0 || hits.length * 3 < lines.length) {
    const fragment: KeywordSearchFragment = {
      fragmentId: 'keyword-search-fallback',
      semanticText: text,
      display: { title: 'Raw ripgrep results — LLM filter unavailable', body: text },
      revealTarget,
      kind: 'fallback',
    };
    return { hits: [], rawFallback: true, empty: false, fallbackText: text, fragments: [fragment] };
  }
  return { hits, rawFallback: false, empty: false, fallbackText: null, fragments: hits.map((hit) => hit.fragment) };
}

export function buildAgentTextFragments(
  blocks: readonly ContentBlock[],
  density: 'full' | 'compact',
  options: { forceExpandedText?: boolean | undefined } = {},
): ConversationTextFragment[] {
  const out: ConversationTextFragment[] = [];
  blocks.forEach((block, index) => {
    if (block.type !== 'text') return;
    const semanticText = block.text ?? '';
    const fragmentId = `agent-text-${index}`;
    const collapsed = !options.forceExpandedText && density === 'compact' && shouldCollapseCompactText(semanticText);
    out.push({
      fragmentId,
      semanticText,
      revealTarget: { kind: 'agent-text', key: fragmentId },
      display: collapsed
        ? { mode: 'compact-collapsed', summaryText: firstLineSummary(semanticText) }
        : { mode: 'full', summaryText: semanticText },
    });
  });
  return out;
}

function agentTurnSources(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
  density: 'full' | 'compact',
  isLatestAgentMessage: boolean,
): Array<{ role: string; text: string; fragmentId?: string; revealTarget?: ConversationFragmentRevealTarget }> {
  const forceExpandedText = isLatestAgentMessage
    || (message.display_data as { forceExpandedText?: boolean } | null | undefined)?.forceExpandedText === true;
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const out: Array<{ role: string; text: string; fragmentId?: string; revealTarget?: ConversationFragmentRevealTarget }> = [];
  for (const fragment of buildAgentTextFragments(blocks, density, { forceExpandedText })) {
    out.push({ role: fragment.fragmentId, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
  }
  blocks.forEach((block, index) => {
    if (block.type === 'tool_use') {
      out.push({ role: `tool-use-name-${index}`, text: block.name ?? '' });
      out.push({ role: `tool-use-display-${index}`, text: block.display ?? '' });
      const detailsVisible = densityToolDetailsVisible(block.name, density);
      if (detailsVisible) out.push({ role: `tool-use-input-${index}`, text: stableJson(block.input) });
      const toolResult = toolResultsByUseId.get(block.id ?? '');
      const resultText = toolResultText(toolResult);
      const subAgentFragments = buildSubAgentCardFragments(toolResult?.display_data, block.id ?? '');
      if (subAgentFragments.length > 0) {
        for (const fragment of subAgentFragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      } else if (block.name === 'keyword_search') {
        for (const fragment of buildKeywordSearchOutputProjection(resultText, block.id ? { toolUseId: block.id } : {}).fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      } else if (block.name === 'search') {
        const searchProjection = buildSearchOutputProjection(resultText, block.id ? { toolUseId: block.id } : {});
        for (const fragment of searchProjection.fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      } else if (block.name === 'read_file') {
        const readFileProjection = buildReadFileOutputProjection(resultText, block.input ?? {}, block.id ? { toolUseId: block.id } : {});
        for (const fragment of readFileProjection.fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      } else if (block.name === 'patch') {
        const displayDiff = (toolResult?.display_data as { diff?: unknown } | null | undefined)?.diff;
        const patchDiff = typeof displayDiff === 'string' ? displayDiff : resultText;
        for (const fragment of buildPatchOutputProjection(patchDiff, block.id ? { toolUseId: block.id } : {}).fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      } else {
        const family: TerminalToolResultFamily = block.name === 'bash' || block.name === 'tmux'
          ? block.name
          : block.name === 'browser_profile'
            ? 'browser-profile'
            : 'opaque';
        for (const fragment of buildTerminalToolResultProjection(family, resultText, toolResult?.display_data, block.id ? { toolUseId: block.id } : {}).fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
      }
    }
  });
  return out;
}

function densityToolDetailsVisible(toolName: string | undefined, density: 'full' | 'compact'): boolean {
  if (toolName === 'think') return true;
  return density === 'full';
}

function containsMermaidFence(text: string): boolean {
  return /```\s*mermaid\b/i.test(text);
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
  buildTarget: (source: TSource, match: ViewerFindMatch, matchOrdinal: number) => TTarget,
): SearchableSourceMatch<TTarget>[] {
  const out: SearchableSourceMatch<TTarget>[] = [];
  let matchOrdinal = 0;
  for (const source of sources) {
    for (const match of findLiteralMatches(source.text, query).matches) {
      out.push({
        sourceId: source.id,
        sourceText: source.text,
        target: buildTarget(source, match, matchOrdinal),
        start: match.start,
        end: match.end,
      });
      matchOrdinal += 1;
    }
  }
  return out;
}
