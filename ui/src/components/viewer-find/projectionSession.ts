import { createMatchId, type FindSessionMatch, type MatchId } from './findSession';
import type { SearchableSourceMatch } from './searchProjections';

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
