import { createMatchId, type FindSessionMatch, type MatchId } from './findSession';
import type { SearchableSourceMatch } from './searchProjections';
import { buildMarkdownDisplayBlocks, type ConversationSearchProjection, type ConversationSearchMatchTarget } from './searchProjections';
import type { RenderUnit } from '../../conversation/renderUnits';

export type MatchIdentity<TTarget> = (match: SearchableSourceMatch<TTarget>) => string;

export function projectionMatchesToSessionMatches<TTarget>(
  matches: readonly SearchableSourceMatch<TTarget>[],
  identity: MatchIdentity<TTarget>,
): readonly FindSessionMatch<TTarget>[] {
  const seen = new Set<string>();
  return matches.map((match) => {
    const value = identity(match);
    if (seen.has(value)) throw new Error(`duplicate viewer find match identity: ${value}`);
    seen.add(value);
    return { id: createMatchId(value), target: match.target };
  });
}

export function activeSessionMatchIndex<TTarget>(
  matches: readonly FindSessionMatch<TTarget>[],
  activeMatchId: MatchId | null,
): number {
  if (activeMatchId === null) return -1;
  return matches.findIndex((match) => match.id === activeMatchId);
}

/** Map only single-text turns and contiguous, unambiguous Markdown text ranges. */
export function conversationProjectionSessionMatches(
  projection: ConversationSearchProjection,
  units: readonly RenderUnit[],
): readonly FindSessionMatch<ConversationSearchMatchTarget>[] {
  if (projection.matches.length === 0) return [];
  const matchedSources = new Set(projection.matches.map((match) => match.sourceId));
  const ranges = new Map<string, { semanticStart: number; semanticEnd: number; rawStart: number }[]>();
  for (const unit of units) {
    if (unit.kind !== 'agent_turn' || !Array.isArray(unit.agent.content)) continue;
    const texts = unit.agent.content.filter((block) => block.type === 'text');
    if (texts.length !== 1) continue;
    const raw = texts[0]!.text ?? '';
    const fragmentIndex = unit.agent.content.indexOf(texts[0]!);
    if (!matchedSources.has(`${unit.key}:agent-text-${fragmentIndex}`)) continue;
    let semanticStart = 0;
    const mapped = [];
    for (const block of buildMarkdownDisplayBlocks(raw, 'conversation')) {
      if (block.kind === 'code' && block.language !== 'mermaid') continue;
      const source = raw.slice(block.sourceRange.start, block.sourceRange.end);
      const offset = source.indexOf(block.searchableText);
      // Links, images, HTML, escapes and entities can hide raw text that happens
      // to equal rendered prose; substring uniqueness cannot prove provenance.
      const hasHiddenOrTransformedText = /[[\]<>\\&`]/.test(source);
      if (!hasHiddenOrTransformedText && offset >= 0 && source.indexOf(block.searchableText, offset + 1) === -1) {
        mapped.push({ semanticStart, semanticEnd: semanticStart + block.searchableText.length, rawStart: block.sourceRange.start + offset });
      }
      semanticStart += block.searchableText.length + 1;
    }
    ranges.set(`${unit.key}:agent-text-${fragmentIndex}`, mapped);
  }
  const sources = new Map(projection.sources.map((source) => [source.id, source]));
  return projectionMatchesToSessionMatches(projection.matches, (match) => {
    const source = sources.get(match.sourceId);
    if (source?.role === 'streaming-agent') {
      return JSON.stringify(['agent-prose', source.unitKey, match.start, match.end]);
    }
    const range = ranges.get(match.sourceId)?.find((candidate) => match.start >= candidate.semanticStart && match.end <= candidate.semanticEnd);
    if (source && range) {
      return JSON.stringify(['agent-prose', source.unitKey, range.rawStart + match.start - range.semanticStart, range.rawStart + match.end - range.semanticStart]);
    }
    return JSON.stringify(['source', match.sourceId, match.start, match.end]);
  });
}
