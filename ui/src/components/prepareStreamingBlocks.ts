import type { Code, Nodes, Root, RootContent } from 'mdast';
import { prepareMarkdown } from './prepareMarkdown';
import { nativeCodeComplete } from './markdown-disclosures/interpret';

export type PreparedStreamingBlock = (
  | { type: 'markdown'; content: string }
  | { type: 'code'; lang: Code['lang']; meta: Code['meta']; content: string; complete: boolean }
) & { tree?: Root; revision?: string };

function annotateStreamingCode(tree: Root): void {
  const stack: Nodes[] = [tree];
  while (stack.length) {
    const parent = stack.pop()!;
    if (!('children' in parent)) continue;
    for (let i = 0; i < parent.children.length; i++) {
      const node = parent.children[i]!;
      if (node.type === 'code') {
        parent.children[i] = { ...node, data: { ...node.data, hProperties: {
          ...node.data?.hProperties,
          'data-streaming-complete': nativeCodeComplete(node),
          'data-streaming-lang': node.lang ?? '',
        } } };
      } else stack.push(node);
    }
  }
}

function hasFootnote(node: Nodes): boolean {
  const stack: Nodes[] = [node];
  while (stack.length) {
    const current = stack.pop()!;
    if (current.type === 'footnoteReference' || current.type === 'footnoteDefinition') return true;
    if ('children' in current) for (const child of current.children) stack.push(child);
  }
  return false;
}

export function prepareStreamingBlocks(source: string): PreparedStreamingBlock[] {
  const tree = prepareMarkdown(source, 'conversation');
  const blocks: PreparedStreamingBlock[] = [];
  let pending: RootContent[] = [];
  const flush = () => {
    if (!pending.length) return;
    const selected: Root = { type: 'root', children: pending };
    annotateStreamingCode(selected);
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
    if (node.type === 'code' && !coupled) {
      flush();
      blocks.push({ type: 'code', content: node.value, lang: node.lang, meta: node.meta, complete: nativeCodeComplete(node) });
      continue;
    }
    pending.push(node);
  }
  flush();
  return blocks;
}

