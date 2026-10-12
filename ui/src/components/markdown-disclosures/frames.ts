import type { AcceptedFrame, InterpretationStats, RecognizedTag, SourceRange } from './types';

type Phase =
  | { kind: 'summaryOpen' }
  | { kind: 'summary'; summaryStart: number; summaryOpenStart: number }
  | { kind: 'body'; summaryStart: number; summaryOpenStart: number; summaryEnd: number; bodyStart: number };

interface Frame {
  start: number;
  openEnd: number;
  open: boolean;
  phase: Phase;
  close: { end: number; bodyEnd: number } | null;
  invalid: boolean;
  children: Frame[];
  openingContext: 'body' | 'provisional';
}

export function acceptFrames(source: string, tags: RecognizedTag[], stats: InterpretationStats) {
  const frames: Frame[] = [];
  const roots: Frame[] = [];
  for (const tag of tags) {
    stats.frameSteps++;
    const current = frames.at(-1);
    const grammar = tag.grammar;
    if (!grammar.valid) {
      if (current) current.invalid = true;
      continue;
    }
    if (grammar.kind === 'details' && !grammar.closing) {
      const frame: Frame = {
        start: tag.start, openEnd: tag.end, open: grammar.open,
        phase: { kind: 'summaryOpen' }, close: null, invalid: false, children: [],
        openingContext: !current || current.phase.kind === 'body' ? 'body' : 'provisional',
      };
      if (current) current.children.push(frame);
      else roots.push(frame);
      frames.push(frame);
    } else if (grammar.kind === 'summary' && !grammar.closing) {
      if (current?.phase.kind === 'summaryOpen') {
        current.phase = { kind: 'summary', summaryStart: tag.end, summaryOpenStart: tag.start };
        if (current.children.length || !/^\s*$/.test(source.slice(current.openEnd, tag.start))) current.invalid = true;
      } else if (current) current.invalid = true;
    } else if (grammar.kind === 'summary') {
      if (current?.phase.kind === 'summary') {
        current.phase = { ...current.phase, kind: 'body', summaryEnd: tag.start, bodyStart: tag.end };
      } else if (current) current.invalid = true;
    } else if (current) {
      current.close = { end: tag.end, bodyEnd: tag.start };
      frames.pop();
    }
  }

  const accepted: AcceptedFrame[] = [];
  const ranges: SourceRange[] = [];
  const todo = roots.reverse();
  while (todo.length) {
    stats.frameSteps++;
    const frame = todo.pop()!;
    if (frame.close) {
      if (!frame.invalid && frame.phase.kind === 'body' && !frame.children.length) {
        ranges.push({ start: frame.start, end: frame.close.end });
        accepted.push({
          start: frame.start, end: frame.close.end, open: frame.open,
          summaryOpenStart: frame.phase.summaryOpenStart,
          summaryStart: frame.phase.summaryStart, summaryEnd: frame.phase.summaryEnd,
          bodyStart: frame.phase.bodyStart, bodyEnd: frame.close.bodyEnd,
        });
      }
    } else if (frame.phase.kind === 'body') {
      for (let i = frame.children.length - 1; i >= 0; i--) {
        const child = frame.children[i]!;
        if (child.openingContext === 'body') todo.push(child);
      }
    }
  }
  return { accepted, ranges };
}
