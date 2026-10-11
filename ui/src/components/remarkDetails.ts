import remarkParse from 'remark-parse';
import { unified, type Plugin } from 'unified';

const parser = unified().use(remarkParse);
type MarkdownRoot = ReturnType<typeof parser.parse>;
type MarkdownNode = MarkdownRoot['children'][number];
type Node = {
  type: string;
  value?: string;
  children?: Node[];
  position?: MarkdownNode['position'];
};

// Quoted attribute values may contain >; no attribute is copied into the tree.
const ATTRIBUTES = String.raw`((?:"[^"]*"|'[^']*'|[^'">])*)`;
const HEADER = new RegExp(String.raw`^<details(?=\s|>)${ATTRIBUTES}>\s*<summary(?=\s|>)${ATTRIBUTES}>([\s\S]*?)<\/summary\s*>`, 'i');
const OPENING = /^<details(?:\s|>)/i;
const CLOSING = /^<\/details\s*>/i;
const SUPPORTED_TAGS = /<\/?(?:details|summary)\b[^>]*>/gi;

function hasOpenAttribute(attributes: string): boolean {
  const tokens = attributes.matchAll(/([^\s=]+)(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s]+))?/g);
  return Array.from(tokens).some((token) => token[1]?.toLowerCase() === 'open');
}

function* htmlNodes(node: Node): Generator<Node> {
  if (node.type === 'html') yield node;
  else for (const child of node.children ?? []) yield* htmlNodes(child);
}

function upperBound(offsets: number[], offset: number): number {
  let low = 0;
  let high = offsets.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (offsets[middle]! <= offset) low = middle + 1;
    else high = middle;
  }
  return low;
}

function sourcePoints(source: string) {
  const starts = [0];
  for (let index = 0; index < source.length; index++) {
    if (source[index] === '\n') starts.push(index + 1);
    else if (source[index] === '\r' && source[index + 1] !== '\n') starts.push(index + 1);
  }
  return (offset: number) => {
    const line = upperBound(starts, offset);
    return { offset, line, column: offset - starts[line - 1]! + 1 };
  };
}

/** Mask disclosure wrappers so CommonMark can identify code inside their bodies. */
function disclosureCodeRegions(source: string, parse: (text: string) => MarkdownRoot): (offset: number) => boolean {
  const masked = source.replace(/<\/?(?:details|summary)\b(?:"[^"]*"|'[^']*'|[^'">])*>/gi,
    (tag) => tag.replace(/[^\r\n]/g, 'x'));
  const excluded: { start: number; end: number }[] = [];
  const visit = (node: Node) => {
    if (node.type === 'code' || node.type === 'inlineCode') {
      const start = node.position?.start.offset;
      const end = node.position?.end.offset;
      if (start !== undefined && end !== undefined) excluded.push({ start, end });
    } else node.children?.forEach(visit);
  };
  visit(parse(masked));
  const starts = excluded.map((range) => range.start);
  return (offset) => {
    const range = excluded[upperBound(starts, offset) - 1];
    return range !== undefined && offset < range.end;
  };
}

function disclosureStarts(source: string, isCode: (offset: number) => boolean): number[] {
  return Array.from(source.matchAll(new RegExp(String.raw`<details(?=\s|>)${ATTRIBUTES}>`, 'gi')))
    .filter((match) => !isCode(match.index))
    .map((match) => match.index);
}

function relocate(node: Node, pointAt: ReturnType<typeof sourcePoints>, offset: number, insertedEscapes: number[] = []): void {
  const visit = (current: Node) => {
    if (current.position) {
      for (const point of [current.position.start, current.position.end]) {
        if (point.offset === undefined) continue;
        const originalOffset = point.offset - upperBound(insertedEscapes, point.offset - 1);
        Object.assign(point, pointAt(offset + originalOffset));
      }
    }
    current.children?.forEach(visit);
  };
  visit(node);
}

function normalizeIdentifier(label: string): string {
  return label.replace(/[\t\n\r ]+/g, ' ').replace(/^ | $/g, '').toLowerCase().toUpperCase();
}

function referenceContext(source: string, tree: MarkdownRoot): (content: string) => string {
  const definitions = new Map<string, string>();
  const collect = (node: MarkdownNode) => {
    if (node.type === 'definition') {
      const start = node.position?.start.offset;
      const end = node.position?.end.offset;
      const identifier = normalizeIdentifier(node.identifier);
      if (start !== undefined && end !== undefined && !definitions.has(identifier)) {
        definitions.set(identifier, source.slice(start, end));
      }
    } else if ('children' in node) node.children.forEach(collect);
  };
  tree.children.forEach(collect);
  return (content) => {
    const referenced = new Set<string>();
    let opening: number | undefined;
    // A conservative scan includes both labels of full references, and the
    // single label of shortcut/collapsed references, even in code or HTML.
    for (let index = 0; index < content.length; index++) {
      if (content[index] === '\\') index++;
      else if (content[index] === '[') opening = index + 1;
      else if (content[index] === ']' && opening !== undefined) {
        const identifier = normalizeIdentifier(content.slice(opening, index));
        if (definitions.has(identifier)) referenced.add(identifier);
        opening = undefined;
      }
    }
    return referenced.size
      ? `${content}\n\n${Array.from(referenced, (identifier) => definitions.get(identifier)!).join('\n')}`
      : content;
  };
}

/** Parse document definitions once, then supply only a chunk's referenced labels. */
export function markdownReferenceContext(source: string): (content: string) => string {
  return referenceContext(source, parser.parse(source));
}

/**
 * Basic top-level disclosures only. Markdown is parsed by the owning processor;
 * ordinary HTML stays HTML and never enters a raw-HTML rendering path.
 */
export const remarkDetails: Plugin<[], MarkdownRoot> = function remarkDetails() {
  const parse = (source: string) => this.parse(source) as MarkdownRoot;
  return (tree, file) => {
    const source = String(file);
    const pointAt = sourcePoints(source);
    const isCode = disclosureCodeRegions(source, parse);
    const starts = disclosureStarts(source, isCode);
    const withReferences = referenceContext(source, tree);
    const parseContent = (content: string) => {
      const parsed = parse(withReferences(content));
      parsed.children = parsed.children.filter((node) => (node.position?.start.offset ?? 0) < content.length);
      return parsed;
    };
    type Work = { node: MarkdownNode } | { start: number; end: number };
    const pending: Work[] = tree.children.map((node) => ({ node })).reverse();
    const children: MarkdownNode[] = [];
    while (pending.length) {
      const work = pending.pop()!;
      if (!('node' in work)) {
        // Split a swallowed HTML tail before the second upcoming opening, so its
        // first disclosure is parsed whole without repeatedly parsing the suffix.
        const next = upperBound(starts, work.start - 1);
        const end = Math.min(work.end, starts[next + 1] ?? work.end);
        if (end < work.end) pending.push({ start: end, end: work.end });
        const tail = parseContent(source.slice(work.start, end));
        tail.children.forEach((child) => relocate(child, pointAt, work.start));
        for (let index = tail.children.length - 1; index >= 0; index--) pending.push({ node: tail.children[index]! });
        continue;
      }
      const node = work.node;
      if (node.type !== 'html' || !OPENING.test(node.value)) {
        children.push(node);
        continue;
      }
      const start = node.position?.start.offset;
      if (start === undefined) {
        children.push(node);
        continue;
      }
      const regionEnd = starts[upperBound(starts, start)] ?? source.length;
      const remaining = source.slice(start, regionEnd);
      const header = HEADER.exec(remaining);
      let end: number | undefined;
      let bodyEnd: number | undefined;
      const summaryTree = header ? parseContent(header[3]!) : undefined;
      if (header && summaryTree && !Array.from(htmlNodes(summaryTree)).some((html) => /<\/?(?:details|summary)\b/i.test(html.value ?? ''))) {
        const bodyStart = start + header[0].length;
        const bodyTree = parse(source.slice(bodyStart, regionEnd));
        for (const html of htmlNodes(bodyTree)) {
          if (OPENING.test(html.value ?? '')) break; // Nested disclosures are outside the basic syntax.
          const closing = CLOSING.exec(html.value ?? '');
          if (closing && html.position?.start.offset !== undefined) {
            bodyEnd = bodyStart + html.position.start.offset;
            end = bodyEnd + closing[0].length;
            break;
          }
        }
        if (end !== undefined && bodyEnd !== undefined) {
          const summarySource = header[3]!;
          const summaryStart = bodyStart - /<\/summary\s*>$/i.exec(header[0])![0].length - summarySource.length;
          const paragraph = summaryTree.children[0];
          // A summary is phrasing content, not a second block document.
          const summary: Extract<MarkdownNode, { type: 'paragraph' }> = {
            type: 'paragraph',
            children: summaryTree.children.length === 1 && paragraph?.type === 'paragraph'
              ? paragraph.children
              : [{ type: 'text', value: summarySource.trim() || 'Details' }],
            data: { hName: 'summary' },
          };
          summary.children.forEach((child) => relocate(child, pointAt, summaryStart));
          summary.position = { start: pointAt(summaryStart), end: pointAt(bodyStart) };
          const body = parseContent(source.slice(bodyStart, bodyEnd));
          body.children.forEach((child) => relocate(child, pointAt, bodyStart));
          const details: Extract<MarkdownNode, { type: 'blockquote' }> = {
            type: 'blockquote',
            children: [summary, ...body.children] as Extract<MarkdownNode, { type: 'blockquote' }>['children'],
            data: { hName: 'details', hProperties: { open: hasOpenAttribute(header[1]!) } },
            position: { start: pointAt(start), end: pointAt(end) },
          };
          let tailEnd = node.position?.end.offset ?? end;
          while (tailEnd < end && pending.length) {
            const consumed = pending.pop()!;
            tailEnd = 'node' in consumed ? consumed.node.position?.end.offset ?? end : consumed.end;
          }
          if (tailEnd > end) pending.push({ start: end, end: tailEnd });
          children.push(details);
          continue;
        }
      }
      // Recover Markdown that CommonMark bundled into an unclosed HTML block.
      const insertedEscapes: number[] = [];
      const original = source.slice(start, node.position?.end.offset);
      const escaped = original.replace(SUPPORTED_TAGS, (tag, offset: number) => {
        if (isCode(start + offset)) return tag;
        insertedEscapes.push(offset + insertedEscapes.length);
        return `\\${tag}`;
      });
      const recovered = parse(escaped);
      recovered.children.forEach((child) => relocate(child, pointAt, start, insertedEscapes));
      children.push(...recovered.children);
    }
    tree.children = children;
  };
};

parser.use(remarkDetails);

/** Only complete disclosures are kept together by the streaming fence splitter. */
export function markdownDetailsRanges(source: string): { start: number; end: number }[] {
  if (!/<details(?:\s|>)/i.test(source)) return [];
  const tree = parser.runSync(parser.parse(source), { value: source }) as MarkdownRoot;
  return tree.children.flatMap((node) => {
    const start = node.position?.start.offset;
    const end = node.position?.end.offset;
    return node.data?.hName === 'details' && start !== undefined && end !== undefined ? [{ start, end }] : [];
  });
}
