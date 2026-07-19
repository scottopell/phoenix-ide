import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import { unified } from 'unified';

import type { ContentBlock, Message, ToolResultContent } from '../../api';
import type { BashToolProgress } from '../../generated/sse';
import type { StreamingBuffer } from '../../conversation/atom';
import type { DiffSection } from '../../contexts/ReviewNotesContext';
import type { QueuedMessage } from '../../hooks/useMessageQueue';
import type { RenderUnit } from '../../conversation/renderUnits';
import { buildSectionItems, lineTextAt as diffLineTextAt } from '../viewer/pierreDiffMapping';
import { findLiteralMatches, type ViewerFindMatch } from './literalMatch';
import { formatToolInput, skillResultVisibleText } from '../toolInputDisplay';
import { buildCommissionReviewInlineSearchFragments, parseCommissionReviewResult } from '../../features/commissionReview/model';
import { buildBrowserProfileVisibleText } from '../BrowserProfileResponseView';

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
  sourceText: string;
}

export interface SearchResultRevealTarget {
  kind: 'tool-result-search';
  key: string;
  toolUseId: string;
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

export interface ReadFileNoteFragmentDisplay {
  note: string;
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
    }
  | {
      fragmentId: string;
      semanticText: string;
      display: ReadFileNoteFragmentDisplay;
      revealTarget: ReadFileRevealTarget;
      kind: 'note';
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

export type TerminalToolResultFamily = 'bash' | 'tmux' | 'browser-profile' | 'console-logs' | 'opaque';

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
  toolUseId: string;
}

export interface MessageAttachmentRevealTarget {
  kind: 'message-attachment';
  fragmentId: string;
}

export interface MessageTextRevealTarget {
  kind: 'message-text';
  fragmentId: string;
}

export interface BrowserProfileResultRevealTarget {
  kind: 'tool-result-browser-profile';
  toolUseId: string;
  fragmentId: string;
}

export interface CommissionReviewResultRevealTarget {
  kind: 'tool-result-commission-review';
  toolUseId: string;
  fragmentId: string;
}

export interface ToolUseInputRevealTarget {
  kind: 'tool-use-input';
  toolUseId: string;
  fragmentId: string;
}

export interface ConversationTextFragmentRevealTarget {
  kind: 'agent-text';
  key: string;
}

export type ConversationFragmentRevealTarget =
  | ConversationTextFragmentRevealTarget
  | MessageTextRevealTarget
  | MessageAttachmentRevealTarget
  | ToolUseInputRevealTarget
  | BrowserProfileResultRevealTarget
  | CommissionReviewResultRevealTarget
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
  kind: 'hit' | 'fallback' | 'empty' | 'note';
  path?: string;
  explanation?: string;
}

export interface KeywordSearchOutputProjection {
  hits: Array<{ path: string; explanation: string; fragment: KeywordSearchFragment }>;
  notes: Array<{ text: string; fragment: KeywordSearchFragment }>;
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
  kind: 'group' | 'hit' | 'note' | 'empty' | 'fallback';
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
  fragment: SearchResultFragment;
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
  fragmentId: 'system-prompt-text';
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
  lang?: string;
}

export interface MarkdownDisplayBlock {
  id: string;
  lineNumber: number;
  sourceRange: { start: number; end: number };
  searchableText: string;
  kind: string;
  language?: string;
}

function renderedMarkdownText(node: MarkdownNode): string {
  if (node.type === 'image' || node.type === 'imageReference') return '';
  if (typeof node.value === 'string') return node.value;
  return node.children?.map(renderedMarkdownText).join('') ?? '';
}

export function buildMarkdownDisplayBlocks(markdown: string): readonly MarkdownDisplayBlock[] {
  const processor = unified().use(remarkParse).use(remarkGfm);
  const tree = processor.runSync(processor.parse(markdown)) as MarkdownNode;
  const blocks: MarkdownDisplayBlock[] = [];
  const visit = (node: MarkdownNode, path: readonly number[]) => {
    if (node.type !== 'root' && node.position?.start?.line && node.position.start.offset !== undefined && node.position.end?.offset !== undefined) {
      const isDisplayBlock = ['paragraph', 'heading', 'tableCell', 'code'].includes(node.type);
      if (isDisplayBlock) {
        const searchableText = renderedMarkdownText(node);
        if (searchableText) {
          blocks.push({
            id: `markdown:${path.join('.')}:${node.position.start.offset}-${node.position.end.offset}`,
            lineNumber: node.position.start.line,
            sourceRange: { start: node.position.start.offset, end: node.position.end.offset },
            searchableText,
            kind: node.type,
            ...(node.lang ? { language: node.lang } : {}),
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

export function buildMarkdownFileSearchProjection(content: string, query: string): FileSearchProjection {
  const sources: FileSearchSource[] = buildMarkdownDisplayBlocks(content)
    .filter((block) => block.kind !== 'code')
    .map((block) => ({
    id: `markdown:${block.id}`,
    kind: 'line',
    lineNumber: block.lineNumber,
    text: block.searchableText,
    target: { kind: 'file-line', lineNumber: block.lineNumber, startColumn: 0, endColumn: 0 },
  }));
  return { sources, matches: projectMatches(sources, query, (source, match, matchOrdinal) => ({
    kind: 'file-line',
    lineNumber: source.lineNumber,
    startColumn: match.start,
    endColumn: match.end,
    matchOrdinal,
  })) };
}

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
  commissionReviewCanOpenFullReview?: boolean;
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
        addConversationMessageSources(sources, unitIndex, unit.kind, unit.key, 'user-message', userMessageParts(unit.message));
        break;
      case 'pending_user':
        addConversationMessageSources(sources, unitIndex, unit.kind, unit.key, 'pending-user-message', queuedMessageParts(unit.message));
        break;
      case 'skill':
        addConversationMessageSources(sources, unitIndex, unit.kind, unit.key, 'skill-message', skillMessageParts(unit.message));
        break;
      case 'system':
        if (!isHiddenSystemMessage(unit.message)) {
          addConversationMessageSources(sources, unitIndex, unit.kind, unit.key, 'system-message', userMessageParts(unit.message));
        }
        break;
      case 'agent_turn':
        for (const source of agentTurnSources(
          unit.agent,
          unit.toolResultsByUseId,
          density,
          unit.key === options.latestAgentKey,
          options.commissionReviewCanOpenFullReview === true,
          options.liveBashProgress ?? {},
        )) {
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
          fragmentId: source.target.fragmentId,
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
    fragmentId: 'system-prompt-text',
    text,
    target: {
      kind: 'header-text',
      headerKey,
      fragmentId: 'system-prompt-text',
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

function addConversationMessageSources(
  sources: ConversationSearchSource[],
  unitIndex: number,
  unitKind: RenderUnit['kind'],
  unitKey: string,
  role: string,
  parts: { text: string; attachments: string[] },
): void {
  if (parts.text) addConversationSource(sources, unitIndex, unitKind, unitKey, role, parts.text, 'message-text', { kind: 'message-text', fragmentId: 'message-text' });
  parts.attachments.forEach((name, index) => {
    const fragmentId = `message-attachment-${index}`;
    addConversationSource(sources, unitIndex, unitKind, unitKey, `${role}-attachment-${index}`, name, fragmentId, { kind: 'message-attachment', fragmentId });
  });
}

function userMessageParts(message: Message): { text: string; attachments: string[] } {
  if (typeof message.content === 'string') return { text: message.content, attachments: [] };
  const content = message.content as { text?: string; files?: Array<{ original_name?: string }> };
  return {
    text: typeof content.text === 'string' ? content.text : '',
    attachments: (content.files ?? []).flatMap((file) => typeof file.original_name === 'string' && file.original_name ? [file.original_name] : []),
  };
}

function skillMessageParts(message: Message): { text: string; attachments: string[] } {
  const content = message.content as { name?: string; trigger?: string; args?: string; files?: Array<{ original_name?: string }> };
  const text = typeof content.trigger === 'string' && content.trigger.trim()
    ? content.trigger.trim()
    : [content.name ? `/${content.name}` : '', typeof content.args === 'string' ? content.args.trim() : ''].filter(Boolean).join(' ');
  return {
    text,
    attachments: (content.files ?? []).flatMap((file) => typeof file.original_name === 'string' && file.original_name ? [file.original_name] : []),
  };
}

function queuedMessageParts(message: QueuedMessage): { text: string; attachments: string[] } {
  return { text: message.text, attachments: (message.files ?? []).map((file) => file.original_name) };
}

function isHiddenSystemMessage(message: Message): boolean {
  const displayData = message.display_data as { hidden?: boolean } | null | undefined;
  return displayData?.hidden === true;
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

function isReadFilePaginationNote(line: string): boolean {
  return /^\[\d+ more lines? not shown\.(?: Use offset=\d+ to continue\.)?\]$/.test(line.trim());
}

function boundedFragmentHash(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
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
  const hasStructuredDisplay = options.toolUseId !== undefined && options.toolUseId !== null;
  if (path.length > 0 && hasStructuredDisplay) {
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
  const duplicateNoteCounts = new Map<string, number>();
  const startLine = typeof input['offset'] === 'number' ? input['offset'] : 1;
  const rawLines = text.split('\n');
  const hasNumberedLines = rawLines.some((line) => parseReadFileRenderedLine(line) !== null);
  for (const [lineIndex, rawLine] of rawLines.entries()) {
    const parsedLine = parseReadFileRenderedLine(rawLine);
    if (hasNumberedLines && !parsedLine) {
      const note = rawLine.trim();
      if (!note || isReadFilePaginationNote(note)) continue;
      const duplicateIndex = duplicateNoteCounts.get(note) ?? 0;
      duplicateNoteCounts.set(note, duplicateIndex + 1);
      const fragmentId = `read-file-note:${encodeURIComponent(note)}:${duplicateIndex}`;
      fragments.push({
        fragmentId,
        semanticText: note,
        display: { note },
        revealTarget: { ...revealBase, fragmentId },
        kind: 'note',
      });
      continue;
    }
    if (!hasNumberedLines && rawLine === '' && lineIndex === rawLines.length - 1) continue;
    const renderedLine = parsedLine ?? {
      lineNumber: startLine + lineIndex,
      content: rawLine,
    };
    const duplicateIndex = duplicateLineCounts.get(renderedLine.content) ?? 0;
    duplicateLineCounts.set(renderedLine.content, duplicateIndex + 1);
    const fragmentId = `read-file-line:${boundedFragmentHash(renderedLine.content)}:${duplicateIndex}`;
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
    if (typeof value[key] === 'string' && value[key].length > 0) return value[key];
  }
  switch (value['type']) {
    case 'success': return 'Completed successfully';
    case 'failure': return 'Failed';
    case 'timed_out': return 'Timed out: sub-agent exceeded its time limit';
    default: return '';
  }
}

export function buildTerminalToolResultProjection(
  family: TerminalToolResultFamily,
  resultText: string,
  displayData: unknown,
  options: { toolUseId?: string | null } = {},
): { fragments: readonly [TerminalToolResultFragment]; fullText: string } {
  const semanticText = family === 'browser-profile' && displayData
    ? semanticObjectText(displayData)
    : family === 'console-logs'
      ? semanticConsoleLogsText(resultText)
      : family === 'bash' || family === 'tmux'
        ? semanticStructuredResultText(resultText, family)
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

function semanticConsoleLogsText(resultText: string): string {
  const trimmed = resultText.trim();
  if (trimmed.startsWith('Logs written to ')) return trimmed;
  try {
    const parsed = JSON.parse(resultText);
    if (!Array.isArray(parsed)) return resultText;
    const entries = parsed.filter((entry): entry is { level: string; text: string } =>
      entry !== null
      && typeof entry === 'object'
      && typeof (entry as { level?: unknown }).level === 'string'
      && typeof (entry as { text?: unknown }).text === 'string');
    if (entries.length === 0) return '(no console entries)';
    const counts = new Map<string, number>();
    for (const entry of entries) counts.set(entry.level, (counts.get(entry.level) ?? 0) + 1);
    const header = [
      `${entries.length} entr${entries.length === 1 ? 'y' : 'ies'}`,
      ...['error', 'warning', 'info', 'log', 'debug']
        .filter((level) => counts.has(level))
        .map((level) => `${counts.get(level)} ${level}`),
    ];
    return [...header, ...entries.flatMap((entry) => [entry.level, entry.text])].join('\n');
  } catch {
    return resultText;
  }
}

function semanticStructuredResultText(resultText: string, family: 'bash' | 'tmux'): string {
  try {
    const parsed = JSON.parse(resultText);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return resultText;
    const record = parsed as Record<string, unknown>;
    if (typeof record['error'] === 'string') {
      const messageKey = family === 'bash' ? 'error_message' : 'message';
      return [
        `error: ${record['error']}`,
        typeof record[messageKey] === 'string' ? record[messageKey] : '',
        family === 'bash' && typeof record['hint'] === 'string' ? record['hint'] : '',
      ].filter(Boolean).join('\n');
    }

    const visible: string[] = [];
    const status = typeof record['status'] === 'string' ? record['status'] : '';
    if (family === 'bash') {
      const statusLabel = status === 'still_running'
        ? 'still running'
        : status === 'kill_pending_kernel'
          ? 'kill pending (kernel)'
          : status === 'tombstoned' && typeof record['final_cause'] === 'string'
            ? `tombstoned · ${record['final_cause']}`
            : status;
      if (statusLabel) visible.push(statusLabel);
      if (typeof record['handle'] === 'string') visible.push(record['handle']);
      if (typeof record['label'] === 'string') visible.push(record['label']);
      if ((status === 'exited' || status === 'tombstoned') && record['exit_code'] !== null && record['exit_code'] !== undefined) {
        visible.push(`exit code ${String(record['exit_code'])}`);
      }
      if ((status === 'killed' || status === 'tombstoned') && typeof record['signal_number'] === 'number') {
        visible.push(`signal ${String(record['signal_number'])}`);
      }
      if (typeof record['kill_signal_sent'] === 'string') visible.push(`kill: ${record['kill_signal_sent']}`);
      if (typeof record['signal_sent'] === 'string' && record['signal_sent'] !== record['kill_signal_sent']) {
        visible.push(`signal_sent: ${record['signal_sent']}`);
      }
      if (typeof record['waited_ms'] === 'number') visible.push(`waited ${Math.round(record['waited_ms'])} ms`);
      if (typeof record['duration_ms'] === 'number') visible.push(`duration ${Math.round(record['duration_ms'])} ms`);
      if (record['truncated_before'] === true) visible.push('[output truncated before this view]');
      if (Array.isArray(record['lines'])) {
        visible.push(record['lines'].map((line) =>
          line && typeof line === 'object' && typeof (line as { bytes?: unknown }).bytes === 'string'
            ? (line as { bytes: string }).bytes
            : '').join('\n'));
      }
    } else {
      if (status) visible.push(status);
      if (record['exit_code'] !== null && record['exit_code'] !== undefined) visible.push(`exit code ${String(record['exit_code'])}`);
      if (typeof record['duration_ms'] === 'number') visible.push(`${Math.round(record['duration_ms'])} ms`);
      if (typeof record['stdout'] === 'string' && record['stdout']) visible.push(`stdout\n${record['stdout']}`);
      if (typeof record['stderr'] === 'string' && record['stderr']) visible.push(`stderr\n${record['stderr']}`);
      if (record['truncated'] === true) visible.push('[output truncated]');
    }
    return visible.filter(Boolean).join('\n');
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
  if (typeof value === 'number' && label) {
    const formatted = formatProfileMetricValue(label, value);
    return formatted === String(value)
      ? `${label}: ${formatted}`
      : `${label}: ${formatted}\n${label}: ${String(value)}`;
  }
  return label ? `${label}: ${String(value)}` : String(value);
}

function formatProfileMetricValue(label: string, value: number): string {
  if (/HeapUsedSize|HeapTotalSize|Size$/i.test(label)) {
    if (Math.abs(value) >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
    if (Math.abs(value) >= 1024) return `${(value / 1024).toFixed(1)} KB`;
    return `${value} B`;
  }
  if (/Duration|Time$/i.test(label)) return `${value.toFixed(3)} s`;
  return Number.isInteger(value) ? value.toLocaleString() : value.toFixed(3);
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
    toolUseId: options.toolUseId ?? '',
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
      const semanticText = `${lineNumber}: ${content}`;
      const identityText = `${path}:${semanticText}`;
      const duplicateIndex = duplicateCounts.get(identityText) ?? 0;
      duplicateCounts.set(identityText, duplicateIndex + 1);
      const fragment: SearchResultFragment = {
        fragmentId: `search-hit:${encodeURIComponent(path)}:${lineNumber}:${boundedFragmentHash(identityText)}:${duplicateIndex}`,
        semanticText,
        display: { path: '', lineNumber, content },
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
  const pathCounts = new Map<string, number>();
  for (const hit of hits) {
    const idx = seen.get(hit.path);
    if (idx === undefined) {
      seen.set(hit.path, groups.length);
      const duplicateIndex = pathCounts.get(hit.path) ?? 0;
      pathCounts.set(hit.path, duplicateIndex + 1);
      const fragment: SearchResultFragment = {
        fragmentId: `search-group:${encodeURIComponent(hit.path)}:${duplicateIndex}`,
        semanticText: hit.path,
        display: { path: hit.path },
        revealTarget: { ...revealBase, path: hit.path },
        kind: 'group',
      };
      groups.push({ path: hit.path, hits: [hit], fragment });
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
      ...groups.flatMap((group) => [group.fragment, ...group.hits.map((hit) => hit.fragment)]),
      ...notes.map((note) => note.fragment),
    ],
  };
}

export function buildKeywordSearchOutputProjection(
  text: string,
  options: { toolUseId?: string | null } = {},
): KeywordSearchOutputProjection {
  const revealTarget = {
    kind: 'tool-result-keyword-search' as const,
    key: keywordSearchToolResultKey(options.toolUseId ?? ''),
    toolUseId: options.toolUseId ?? '',
  };
  const noteCounts = new Map<string, number>();
  const notes: Array<{ text: string; fragment: KeywordSearchFragment }> = [];
  const lines: string[] = [];
  for (const rawLine of text.split('\n')) {
    const line = rawLine.trim();
    if (!line) continue;
    if (line.startsWith('[') && line.endsWith(']')) {
      const noteText = line.slice(1, -1).trim();
      const duplicateIndex = noteCounts.get(noteText) ?? 0;
      noteCounts.set(noteText, duplicateIndex + 1);
      const fragment: KeywordSearchFragment = {
        fragmentId: `keyword-search-note:${encodeURIComponent(noteText)}:${duplicateIndex}`,
        semanticText: noteText,
        display: { title: noteText },
        revealTarget,
        kind: 'note',
      };
      notes.push({ text: noteText, fragment });
    } else {
      lines.push(rawLine);
    }
  }

  const contentText = lines.join('\n');
  const trimmed = contentText.trim();
  const noteFragments = notes.map((note) => note.fragment);
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
    return { hits: [], notes, rawFallback: false, empty: true, fallbackText: null, fragments: [fragment, ...noteFragments] };
  }

  const nonEmptyLines = lines.filter((line) => line.trim());
  const ripgrepShaped = nonEmptyLines.filter((line) => /^[^\s].*?[-:]\d+[-:]/.test(line) || line === '--').length;
  const fallback = (): KeywordSearchOutputProjection => {
    const title = 'Raw ripgrep results — LLM filter unavailable';
    const titleFragment: KeywordSearchFragment = {
      fragmentId: 'keyword-search-fallback-title',
      semanticText: title,
      display: { title },
      revealTarget,
      kind: 'fallback',
    };
    const bodyFragment: KeywordSearchFragment = {
      fragmentId: 'keyword-search-fallback-body',
      semanticText: contentText,
      display: { title, body: contentText },
      revealTarget,
      kind: 'fallback',
    };
    return {
      hits: [],
      notes,
      rawFallback: true,
      empty: false,
      fallbackText: contentText,
      fragments: [titleFragment, bodyFragment, ...noteFragments],
    };
  };
  if (nonEmptyLines.length >= 4 && ripgrepShaped / nonEmptyLines.length > 0.25) return fallback();

  const hits: Array<{ path: string; explanation: string; fragment: KeywordSearchFragment }> = [];
  const duplicateCounts = new Map<string, number>();
  for (const line of nonEmptyLines) {
    const match = /^([^:\s][^:]*?):\s+(.+)$/.exec(line);
    if (!match?.[1] || match[2] === undefined) continue;
    const path = match[1].trim();
    const explanation = match[2].trim();
    const semanticText = `${path}: ${explanation}`;
    const duplicateIndex = duplicateCounts.get(semanticText) ?? 0;
    duplicateCounts.set(semanticText, duplicateIndex + 1);
    hits.push({
      path,
      explanation,
      fragment: {
        fragmentId: `keyword-search-hit:${encodeURIComponent(path)}:${boundedFragmentHash(semanticText)}:${duplicateIndex}`,
        semanticText,
        display: { title: path, body: explanation },
        revealTarget,
        kind: 'hit',
        path,
        explanation,
      },
    });
  }

  if (hits.length === 0 || hits.length * 3 < nonEmptyLines.length) return fallback();
  return {
    hits,
    notes,
    rawFallback: false,
    empty: false,
    fallbackText: null,
    fragments: [...noteFragments, ...hits.map((hit) => hit.fragment)],
  };
}

export function buildAgentTextFragments(
  blocks: readonly ContentBlock[],
  _density: 'full' | 'compact',
  options: { forceExpandedText?: boolean | undefined; forSearch?: boolean | undefined } = {},
): ConversationTextFragment[] {
  const out: ConversationTextFragment[] = [];
  blocks.forEach((block, index) => {
    if (block.type !== 'text') return;
    const sourceText = block.text ?? '';
    const markdownBlocks = options.forSearch
      ? buildMarkdownDisplayBlocks(sourceText).filter((markdownBlock) => markdownBlock.kind !== 'code' || markdownBlock.language === 'mermaid')
      : [];
    const semanticText = options.forSearch
      ? markdownBlocks.map((markdownBlock) => markdownBlock.searchableText).join('\n')
      : sourceText;
    const fragmentId = `agent-text-${index}`;
    out.push({
      fragmentId,
      semanticText,
      revealTarget: { kind: 'agent-text', key: fragmentId },
      display: { mode: 'full', summaryText: semanticText, sourceText },
    });
  });
  return out;
}

const STRUCTURED_BROWSER_PROFILE_ACTIONS = new Set([
  'run_scenario',
  'heap_snapshot',
  'metrics',
  'cpu_stop',
  'cpu_summary',
  'trace_stop',
]);

function toolResultRendersAsImage(toolName: string | undefined, toolResult: Message): boolean {
  const content = toolResult.content as ToolResultContent | undefined;
  if ((content?.images?.length ?? 0) > 0) return true;
  if (toolName !== 'read_image' && toolName !== 'browser_take_screenshot') return false;
  const displayData = toolResult.display_data as { type?: unknown; media_type?: unknown; data?: unknown } | null | undefined;
  return displayData?.type === 'image'
    && typeof displayData.media_type === 'string'
    && typeof displayData.data === 'string';
}

function agentTurnSources(
  message: Message,
  toolResultsByUseId: ReadonlyMap<string, Message>,
  density: 'full' | 'compact',
  isLatestAgentMessage: boolean,
  commissionReviewCanOpenFullReview: boolean,
  liveBashProgress: Readonly<Record<string, { progress: BashToolProgress }>>,
): Array<{ role: string; text: string; fragmentId?: string; revealTarget?: ConversationFragmentRevealTarget }> {
  const forceExpandedText = isLatestAgentMessage
    || (message.display_data as { forceExpandedText?: boolean } | null | undefined)?.forceExpandedText === true;
  const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
  const out: Array<{ role: string; text: string; fragmentId?: string; revealTarget?: ConversationFragmentRevealTarget }> = [];
  const textFragments = new Map(
    buildAgentTextFragments(blocks, 'full', { forceExpandedText, forSearch: true })
      .map((fragment) => [fragment.fragmentId, fragment] as const),
  );
  blocks.forEach((block, index) => {
    if (block.type === 'text') {
      const fragment = textFragments.get(`agent-text-${index}`);
      if (fragment) {
        out.push({ role: fragment.fragmentId, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
      }
      return;
    }
    if (block.type === 'tool_use') {
      const toolUseId = block.id ?? '';
      const inputText = formatToolInput(block.name || 'tool', block.input ?? {}, block.display).display;
      if (inputText && block.name !== 'think') {
        const fragmentId = 'tool-use-input';
        out.push({
          role: `tool-use-input-${index}`,
          text: inputText,
          fragmentId,
          revealTarget: { kind: 'tool-use-input', toolUseId, fragmentId },
        });
      }
      const toolResult = toolResultsByUseId.get(toolUseId);
      if (block.name === 'bash') {
        const visibleText = bashVisibleSearchText(block, toolResult, liveBashProgress[toolUseId]?.progress, density);
        if (visibleText) {
          out.push({
            role: `tool-use-visible-bash-${index}`,
            text: visibleText,
            fragmentId: 'bash-visible',
            revealTarget: { kind: 'tool-result-terminal', toolUseId, fragmentId: 'bash-visible', family: 'bash' },
          });
        }
        return;
      }
      if (!toolResult) return;
      const resultText = toolResultText(toolResult);
      const resultContent = toolResult.content as ToolResultContent | undefined;
      const isError = resultContent?.is_error === true || typeof resultContent?.error === 'string';
      const profileAction = block.name === 'browser_profile' && typeof block.input?.['action'] === 'string'
        ? block.input['action']
        : null;
      const structuredProfile = profileAction !== null && STRUCTURED_BROWSER_PROFILE_ACTIONS.has(profileAction);
      if (!isError && toolResultRendersAsImage(block.name, toolResult)) return;
      if (isError) {
        const errorFamily: TerminalToolResultFamily = block.name === 'bash' || block.name === 'tmux'
          ? block.name
          : structuredProfile
            ? 'browser-profile'
            : 'opaque';
        for (const fragment of buildTerminalToolResultProjection(errorFamily, resultText, toolResult.display_data, block.id ? { toolUseId: block.id } : {}).fragments) {
          out.push({ role: `tool-use-result-${index}:${fragment.fragmentId}`, text: fragment.semanticText, fragmentId: fragment.fragmentId, revealTarget: fragment.revealTarget });
        }
        return;
      }
      if (block.name === 'browser_profile' && structuredProfile) {
        const fragmentId = 'browser-profile-visible';
        const visibleText = buildBrowserProfileVisibleText(profileAction ?? '', toolResult.display_data as Record<string, unknown> | undefined, resultText, isError);
        if (visibleText) {
          out.push({
            role: `browser-profile-${index}`,
            text: visibleText,
            fragmentId,
            revealTarget: { kind: 'tool-result-browser-profile', toolUseId, fragmentId },
          });
        }
        return;
      }
      if (block.name === 'skill') {
        const visibleResult = skillResultVisibleText(resultText);
        if (visibleResult) {
          const fragmentId = 'skill-result-visible';
          out.push({
            role: `skill-result-${index}`,
            text: visibleResult,
            fragmentId,
            revealTarget: { kind: 'tool-result-terminal', toolUseId, fragmentId, family: 'opaque' },
          });
        }
        return;
      }
      if (block.name === 'commission_review') {
        const data = parseCommissionReviewResult(toolResult.display_data, resultText);
        if (data) {
          const renderAllDetails = !(commissionReviewCanOpenFullReview && message.sequence_id !== undefined);
          buildCommissionReviewInlineSearchFragments(data, { renderAllDetails }).forEach((fragment, fragmentIndex) => out.push({
            role: `commission-review-${index}-${fragmentIndex}`,
            text: fragment.text,
            fragmentId: fragment.fragmentId,
            revealTarget: { kind: 'tool-result-commission-review', toolUseId, fragmentId: fragment.fragmentId },
          }));
          return;
        }
      }
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
          : block.name === 'browser_recent_console_logs'
            ? 'console-logs'
            : structuredProfile
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
