export interface ViewerFindMatch {
  start: number;
  end: number;
}

export interface ViewerFindResult {
  query: string;
  haystack: string;
  matches: ViewerFindMatch[];
}

export function findLiteralMatches(haystack: string, query: string): ViewerFindResult {
  if (query.length === 0) {
    return { query, haystack, matches: [] };
  }

  const matches: ViewerFindMatch[] = [];
  let fromIndex = 0;

  while (fromIndex <= haystack.length - query.length) {
    const start = haystack.indexOf(query, fromIndex);
    if (start === -1) break;
    matches.push({ start, end: start + query.length });
    fromIndex = start + 1;
  }

  return { query, haystack, matches };
}
