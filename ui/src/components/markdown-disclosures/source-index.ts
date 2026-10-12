import { preprocess } from 'micromark/phoenix';
import { markdownLineEnding } from 'micromark-util-character';
import type { Point } from 'micromark-util-types';

export function indexSource(source: string) {
  const tape = preprocess()(source, undefined, true);
  const points = new Map<number, Point>();
  const point: Point = {
    _index: 0, _bufferIndex: -1, line: 1, column: 1,
    offset: source.charCodeAt(0) === 65279 ? 1 : 0,
  };
  for (let i = 0; i < tape.length; i++) {
    const chunk = tape[i]!;
    if (typeof chunk === 'string') {
      for (let j = 0; j < chunk.length; j++) {
        points.set(point.offset, { ...point, _index: i, _bufferIndex: j });
        point.column++;
        point.offset++;
      }
    } else {
      points.set(point.offset, { ...point, _index: i, _bufferIndex: -1 });
      if (chunk === null) break;
      if (markdownLineEnding(chunk)) {
        point.line++;
        point.column = 1;
        point.offset += chunk === -3 ? 2 : 1;
      } else if (chunk !== -1) {
        point.column++;
        point.offset++;
      }
    }
  }

  const quoteEnds = new Map<number, number>();
  let single = -1;
  let double = -1;
  for (let i = source.length - 1; i >= 0; i--) {
    if (source[i] === '"') {
      if (double >= 0) quoteEnds.set(i, double);
      double = i;
    } else if (source[i] === "'") {
      if (single >= 0) quoteEnds.set(i, single);
      single = i;
    }
  }
  const blankStarts: number[] = [];
  let lineStart = 0;
  let blank = true;
  for (let i = 0; i <= source.length; i++) {
    const c = source[i];
    if (c === undefined || c === '\r' || c === '\n') {
      if (blank) blankStarts.push(lineStart);
      if (c === '\r' && source[i + 1] === '\n') i++;
      lineStart = i + 1;
      blank = true;
    } else if (c !== ' ' && c !== '\t') blank = false;
  }
  const grammarEnds = new Map<number, number>();
  let nextBlank = 0;
  const candidates = new Map<number, number>();
  const pattern = /<\/?(?:details|summary)(?=[\s/>])/ig;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(source))) {
    candidates.set(match.index, match.index + match[0].length);
    while (nextBlank < blankStarts.length && blankStarts[nextBlank]! <= match.index) nextBlank++;
    grammarEnds.set(match.index, blankStarts[nextBlank] ?? source.length);
  }
  return {
    tape, candidates, quoteEnds, grammarEnds,
    pointAt(offset: number): Point {
      const result = points.get(offset);
      if (!result) throw new Error(`No tokenizer point at source offset ${offset}`);
      return result;
    },
  };
}
