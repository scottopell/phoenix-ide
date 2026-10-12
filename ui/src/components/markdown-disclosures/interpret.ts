import { parse, preprocess, postprocess } from 'micromark/phoenix';
import { compiler } from 'mdast-util-from-markdown/phoenix';
import type { Construct, Event, State, Token, TokenizeContext } from 'micromark-util-types';
import type { Definition, Root, RootContent } from 'mdast';
import { recognizeDisclosures } from './recognize';
import { append, basicOptions, constructList, membership, walk } from './support';
import type {
  DisclosureDetails, MarkdownConfiguration, MarkdownInterpretation, SourceRange,
} from './types';

type ContentPart = SourceRange & (
  | { kind: 'markdown' }
  | { kind: 'summary' | 'body'; frame: number }
);
type Part = ContentPart | (SourceRange & { kind: 'structural'; frame: number });

function sourcePositions(source: string) {
  const positions: { line: number; column: number; offset: number }[] = [];
  let line = 1;
  let column = 1;
  for (let offset = 0; offset <= source.length; offset++) {
    positions.push({ line, column, offset });
    if (source[offset] === '\r') {
      line++;
      column = 1;
    } else if (source[offset] === '\n') {
      if (source[offset - 1] !== '\r') line++;
      column = 1;
    } else column++;
  }
  return (start: number, end: number) => ({ start: positions[start]!, end: positions[end]! });
}

/** Interpret once; rendering, chunk boundaries and Find share this result. */
export function interpretMarkdown(
  source: string,
  options: MarkdownConfiguration = basicOptions,
): MarkdownInterpretation {
  const recognition = recognizeDisclosures(source, options);
  const stats = recognition.stats;
  const position = sourcePositions(source);
  const partition: Part[] = [];
  const frames = recognition.accepted;
  let cursor = 0;
  const add = (part: Part): void => {
    if (part.end > part.start) partition.push(part);
  };
  for (let frame = 0; frame < frames.length; frame++) {
    const current = frames[frame]!;
    add({ kind: 'markdown', start: cursor, end: current.start });
    add({ kind: 'structural', start: current.start, end: current.summaryStart, frame });
    add({ kind: 'summary', start: current.summaryStart, end: current.summaryEnd, frame });
    add({ kind: 'structural', start: current.summaryEnd, end: current.bodyStart, frame });
    add({ kind: 'body', start: current.bodyStart, end: current.bodyEnd, frame });
    add({ kind: 'structural', start: current.bodyEnd, end: current.end, frame });
    cursor = current.end;
  }
  add({ kind: 'markdown', start: cursor, end: source.length });
  if (!source.length) partition.push({ kind: 'markdown', start: 0, end: 0 });

  const recognizedTags = new Map<number, number>();
  for (const tag of recognition.tags) recognizedTags.set(tag.start, tag.end);
  const defined = membership(stats, 'definition');
  const footnotes = membership(stats, 'footnote');

  function parserFor(part: ContentPart) {
    const tags = part.kind === 'markdown' ? recognition.candidates : recognizedTags;
    const parser = parse(options);
    parser.defined = defined;
    parser.gfmFootnotes = footnotes;
    const bom = part.start === 0 && source.charCodeAt(0) === 65279 ? 1 : 0;
    const absolute = (context: TokenizeContext) => part.start + bom + context.now().offset;
    parser.constructs.flow[60] = constructList(parser.constructs.flow[60]).map<Construct>((construct) =>
      construct.name === 'htmlFlow' ? {
        ...construct,
        tokenize(effects, ok, nok) {
          return recognizedTags.has(absolute(this)) ? nok : construct.tokenize.call(this, effects, ok, nok);
        },
      } : construct);
    const textConstructs = constructList(parser.constructs.text[60]);
    parser.constructs.text[60] = textConstructs;
    textConstructs.unshift({
      name: 'disclosureLiteral',
      tokenize(effects, ok, nok) {
        const currentOffset = () => absolute(this);
        const end = tags.get(currentOffset());
        if (end === undefined) return nok;
        const tagEnd = end;
        return start;
        function start(code: Parameters<State>[0]): ReturnType<State> {
          effects.enter('data');
          return consume(code);
        }
        function consume(code: Parameters<State>[0]): ReturnType<State> {
          if (currentOffset() >= tagEnd) {
            effects.exit('data');
            return ok(code);
          }
          if (code === null) return nok(code);
          effects.consume(code);
          return consume;
        }
      },
    });
    return parser;
  }

  const pending = new Map<ContentPart, Event[]>();
  function submit(part: ContentPart): void {
    const parser = parserFor(part);
    const pre = preprocess();
    if (part.start !== 0) pre('');
    const input = source.slice(part.start, part.end);
    stats.submittedUnits += input.length;
    stats.submissions++;
    pending.set(part, (part.kind === 'summary' ? parser.text() : parser.document()).write(pre(input, undefined, true)));
  }
  // Document tokenization collects real definitions before summary references resolve.
  for (const part of partition) if (part.kind === 'markdown' || part.kind === 'body') submit(part);
  for (const part of partition) if (part.kind === 'summary') submit(part);

  const definitions = new Map<string, Definition>();
  const trees = new Map<ContentPart, Root>();
  for (const part of partition) {
    if (part.kind === 'structural') continue;
    const submitted = pending.get(part)!;
    let events = postprocess(submitted);
    stats.committedEvents += events.length;
    if (part.kind === 'summary' && events.length) {
      const first = events[0]!;
      const last = events[events.length - 1]!;
      const token: Token = { type: 'paragraph', start: first[1].start, end: last[1].end };
      const wrapped: Event[] = [['enter', token, first[2]]];
      append(wrapped, events);
      wrapped.push(['exit', token, last[2]]);
      events = wrapped;
    }
    const tree = compiler(options)(events);
    pending.delete(part);
    const bom = part.start === 0 && source.charCodeAt(0) === 65279 ? 1 : 0;
    walk(tree, (node) => {
      if (node.position) {
        const start = node.position.start.offset;
        const end = node.position.end.offset;
        if (start === undefined || end === undefined) throw new Error('Native compiler omitted source offsets');
        node.position = position(part.start + bom + start, part.start + bom + end);
      }
      if (node.type === 'definition' && !definitions.has(node.identifier)) definitions.set(node.identifier, node);
    });
    trees.set(part, tree);
  }

  // Ordinary Markdown retains native reference nodes; disclosure partitions need
  // concrete targets so consumers can render a subtree without injected text.
  if (recognition.tags.length) {
    for (const tree of trees.values()) walk(tree, (parent) => {
      if (!('children' in parent)) return;
      const children: RootContent[] = parent.children;
      for (let i = 0; i < children.length; i++) {
        const child = children[i]!;
        if (child.type !== 'linkReference' && child.type !== 'imageReference') continue;
        const { identifier, label, referenceType, ...rest } = child;
        const definition = definitions.get(identifier);
        if (!definition) throw new Error(`Native ${referenceType} reference ${label ?? identifier} without definition`);
        if (rest.type === 'linkReference') {
          children[i] = { ...rest, type: 'link', url: definition.url, title: definition.title };
        } else {
          children[i] = { ...rest, type: 'image', url: definition.url, title: definition.title };
        }
      }
    });
  }

  const tree: Root = { type: 'root', children: [], position: position(0, source.length) };
  const details = new Map<number, DisclosureDetails>();
  for (const part of partition) {
    if (part.kind === 'markdown') {
      append(tree.children, trees.get(part)!.children);
      continue;
    }
    let disclosure = details.get(part.frame);
    const frame = frames[part.frame]!;
    if (!disclosure) {
      disclosure = {
        type: 'disclosureDetails',
        data: { hName: 'details', hProperties: frame.open ? { open: true } : {} },
        children: [{
          type: 'disclosureSummary', data: { hName: 'summary', hProperties: {} },
          children: [], position: position(frame.summaryOpenStart, frame.bodyStart),
        }],
        position: position(frame.start, frame.end),
      };
      details.set(part.frame, disclosure);
      tree.children.push(disclosure);
    }
    if (part.kind === 'summary') {
      const paragraph = trees.get(part)!.children[0];
      if (paragraph?.type === 'paragraph') append(disclosure.children[0].children, paragraph.children);
    } else if (part.kind === 'body') append<RootContent>(disclosure.children, trees.get(part)!.children);
  }
  return { tree, ranges: recognition.ranges, definitions: Array.from(definitions.values()), stats };
}
