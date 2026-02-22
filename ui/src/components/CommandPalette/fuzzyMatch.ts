/**
 * Simple fuzzy matching with scoring.
 * Scores: exact match > prefix > word-boundary match > substring > character sequence
 */
export function fuzzyMatch<T>(
  items: T[],
  query: string,
  getText: (item: T) => string,
): T[] {
  if (!query) return items;
  const q = query.toLowerCase();

  const scored: { item: T; score: number }[] = [];

  for (const item of items) {
    const text = getText(item).toLowerCase();
    const score = computeScore(text, q);
    if (score > 0) {
      scored.push({ item, score });
    }
  }

  scored.sort((a, b) => b.score - a.score);
  return scored.map(s => s.item);
}

function computeScore(text: string, query: string): number {
  // Exact match
  if (text === query) return 100;

  // Prefix match
  if (text.startsWith(query)) return 80;

  // Word-boundary prefix match (e.g. "bg" matches "background-send")
  const words = text.split(/[-_\s/.]+/);
  if (matchesWordBoundaries(words, query)) return 60;

  // Substring match
  if (text.includes(query)) return 40;

  // Fuzzy character sequence match
  const fuzzyScore = fuzzySequenceScore(text, query);
  if (fuzzyScore > 0) return fuzzyScore;

  return 0;
}

/** Check if query matches first letters of consecutive words */
function matchesWordBoundaries(words: string[], query: string): boolean {
  // Try matching query chars against word starts
  let qi = 0;
  for (const word of words) {
    if (qi >= query.length) break;
    if (word.length > 0 && word[0] === query[qi]) {
      qi++;
    }
  }
  return qi === query.length;
}

/** Score based on how well query chars appear in sequence in text */
function fuzzySequenceScore(text: string, query: string): number {
  let qi = 0;
  let lastMatchPos = -1;
  let consecutiveBonus = 0;

  for (let ti = 0; ti < text.length && qi < query.length; ti++) {
    if (text[ti] === query[qi]) {
      if (lastMatchPos === ti - 1) consecutiveBonus += 5;
      lastMatchPos = ti;
      qi++;
    }
  }

  if (qi < query.length) return 0; // Not all chars matched

  // Base score 20, bonus for consecutive matches, penalty for spread
  const spread = lastMatchPos - (lastMatchPos - qi + 1);
  return Math.max(1, 20 + consecutiveBonus - spread);
}
