export interface ViewerFindMatch {
  start: number;
  end: number;
}

export interface ViewerFindResult {
  query: string;
  haystack: string;
  matches: ViewerFindMatch[];
}

interface FoldedString {
  folded: string;
  mapToSource: number[];
}

function codeUnitWidthAt(text: string, start: number): number {
  const cp = text.codePointAt(start);
  return cp !== undefined && cp > 0xffff ? 2 : 1;
}

function foldForLiteralSearch(text: string): FoldedString {
  const foldedParts: string[] = [];
  const mapToSource: number[] = [];
  for (let i = 0; i < text.length; ) {
    const sourceWidth = codeUnitWidthAt(text, i);
    const sourceSlice = text.slice(i, i + sourceWidth);
    const foldedSlice = sourceSlice.toLocaleLowerCase();
    foldedParts.push(foldedSlice);
    for (let j = 0; j < foldedSlice.length; j++) mapToSource.push(i);
    i += sourceWidth;
  }
  return { folded: foldedParts.join(''), mapToSource };
}

export function findLiteralMatches(haystack: string, query: string): ViewerFindResult {
  if (query.length === 0) {
    return { query, haystack, matches: [] };
  }

  const foldedHaystack = foldForLiteralSearch(haystack);
  const foldedQuery = foldForLiteralSearch(query).folded;
  if (foldedQuery.length === 0) return { query, haystack, matches: [] };

  const matches: ViewerFindMatch[] = [];
  let fromIndex = 0;

  while (fromIndex <= foldedHaystack.folded.length - foldedQuery.length) {
    const start = foldedHaystack.folded.indexOf(foldedQuery, fromIndex);
    if (start === -1) break;
    const foldedEnd = start + foldedQuery.length - 1;
    const sourceStart = foldedHaystack.mapToSource[start]!;
    const sourceEndStart = foldedHaystack.mapToSource[foldedEnd]!;
    const sourceEnd = sourceEndStart + codeUnitWidthAt(haystack, sourceEndStart);
    matches.push({ start: sourceStart, end: sourceEnd });
    fromIndex = start + 1;
  }

  return { query, haystack, matches };
}
