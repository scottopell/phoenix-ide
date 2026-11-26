import type { DiffSection } from '../../contexts/ReviewNotesContext';
import { buildSectionItems, lineTextAt as diffLineTextAt } from '../viewer/pierreDiffMapping';
import {
  projectMatches,
  type DiffSearchProjection,
  type DiffSearchSource,
} from './searchProjections';

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
