import type { Code, Nodes, Root, RootContent } from 'mdast';
import { prepareMarkdown } from './prepareMarkdown';

export type PreparedStreamingBlock = (
  | { type: 'markdown'; content: string }
  | { type: 'code'; lang: Code['lang']; meta: Code['meta']; content: string; complete: boolean }
) & { tree?: Root; revision?: string };

function nativeCodeComplete(node: Code, source: string): boolean {
  const raw = source.slice(node.position?.start.offset, node.position?.end.offset);
  const lines = raw.split(/\r\n|\n|\r/);
  const opener = /^ {0,3}(`{3,}|~{3,})/.exec(lines[0] ?? '');
  if (!opener) return true; // Indented code has no closing delimiter.
  if (lines.length < 2) return false;
  const closer = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(lines[lines.length - 1] ?? '');
  return !!closer && closer[1]![0] === opener[1]![0] && closer[1]!.length >= opener[1]!.length;
}

function hasFootnote(node: Nodes): boolean {
  return node.type === 'footnoteReference' || node.type === 'footnoteDefinition'
    || ('children' in node && node.children.some(hasFootnote));
}

export function prepareStreamingBlocks(source: string): PreparedStreamingBlock[] {
  const tree = prepareMarkdown(source, 'conversation');
  const blocks: PreparedStreamingBlock[] = [];
  let pending: RootContent[] = [];
  const flush = () => {
    if (!pending.length) return;
    const selected: Root = { type: 'root', children: pending };
    const start = pending[0]!.position?.start.offset ?? 0;
    const end = pending[pending.length - 1]!.position?.end.offset ?? source.length;
    blocks.push({ type: 'markdown', content: source.slice(start, end), tree: selected, revision: JSON.stringify(selected) });
    pending = [];
  };
  let coupled = false;
  for (const node of tree.children) {
    if (!coupled && hasFootnote(node)) {
      flush();
      coupled = true;
    }
    if (node.type === 'code') {
      const complete = nativeCodeComplete(node, source);
      if (!coupled) {
        flush();
        blocks.push({ type: 'code', content: node.value, lang: node.lang, meta: node.meta, complete });
        continue;
      }
      pending.push({ ...node, data: { ...node.data, hProperties: {
        ...node.data?.hProperties, 'data-streaming-complete': complete, 'data-streaming-lang': node.lang ?? '',
      } } });
      continue;
    }
    pending.push(node);
  }
  flush();
  return blocks;
}

